//! Streamable HTTP transport implementation

#[cfg(feature = "http-client")]
use http::HeaderMap;

use crate::{
    error::{Error, ErrorCode},
    shared::MemChr,
    types::Message,
};
use futures_util::TryFutureExt;
use std::fmt::Display;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio_util::sync::CancellationToken;

#[cfg(feature = "http-server")]
use std::time::Duration;

use super::{Receiver as TransportReceiver, Sender as TransportSender, Transport};

#[cfg(all(feature = "http-client", feature = "client-tls"))]
use crate::transport::http::client::tls_config::{
    ClientTlsConfig, TlsConfig as McpClientTlsConfig,
};

#[cfg(all(feature = "http-server-volga", feature = "server-tls"))]
pub use volga::tls::{DevCertMode, TlsConfig};

#[cfg(feature = "http-server-volga")]
pub use server::VolgaEngine;

#[cfg(feature = "http-server")]
pub use core::{
    context::HttpContext,
    engine::HttpEngine,
    handlers,
    types::{HttpRequest, HttpResponse, StreamResponse},
};

#[cfg(feature = "http-server")]
#[allow(deprecated)]
pub use core::types::SseResponse;

#[cfg(feature = "http-client")]
pub(crate) mod client;
#[cfg(feature = "http-server")]
pub mod core;
#[cfg(feature = "http-server-volga")]
pub(crate) mod server;

#[cfg(feature = "http-client")]
pub(super) const MCP_SESSION_ID: &str = "Mcp-Session-Id";

/// JSON-RPC method name carried on every outbound HTTP request under
/// MCP 2026-07-28. Allows reverse proxies and load balancers to
/// route without parsing the request body.
///
/// Visible to both sides: the client sends it, and the server must check it
/// against the body it actually dispatches, or an intermediary's routing
/// decision could be made on a method the body never invokes.
#[cfg(not(feature = "legacy-spec"))]
pub(crate) const MCP_METHOD: &str = "Mcp-Method";

/// Entity name -- the tool/prompt name or resource URI -- carried on the
/// requests that have one under MCP 2026-07-28. Validated server-side for
/// the same reason as [`MCP_METHOD`].
#[cfg(not(feature = "legacy-spec"))]
pub(crate) const MCP_NAME: &str = "Mcp-Name";

/// Marks a header value as Base64-encoded UTF-8: `=?base64?{value}?=`.
#[cfg(not(feature = "legacy-spec"))]
pub(crate) const B64_PREFIX: &str = "=?base64?";
/// Closing half of [`B64_PREFIX`].
#[cfg(not(feature = "legacy-spec"))]
pub(crate) const B64_SUFFIX: &str = "?=";

/// Encodes `raw` for use as an HTTP header value, per the spec's value-encoding
/// rules.
///
/// A value travels as it was written when it is visible ASCII, space or
/// horizontal tab, with no leading or trailing whitespace. Anything else -- and
/// any plain value that would otherwise be mistaken for the sentinel -- travels
/// Base64-encoded.
///
/// The safe set is the one the spec states, quoting RFC 9110: visible ASCII
/// (0x21-0x7E), space (0x20) and horizontal tab (0x09). HTAB is in it, so an
/// interior tab is sent as it came; RFC 9110's `field-content` admits `HTAB`
/// between field-vchars, and nothing there lets a recipient rewrite it. The
/// spec's list of reasons to encode -- non-ASCII, control characters,
/// leading/trailing whitespace -- is introduced with "e.g." and describes the
/// ways a value falls outside that set, so it does not withdraw the tab it just
/// admitted. An *edge* tab is still encoded, by the leading/trailing whitespace
/// rule rather than by the safe set.
#[cfg(all(feature = "http-client", not(feature = "legacy-spec")))]
pub(crate) fn encode_header_value(raw: &str) -> String {
    use base64::{Engine, engine::general_purpose::STANDARD};

    let safe = !raw.is_empty()
        && raw
            .bytes()
            .all(|b| b == b'\t' || (0x20..=0x7E).contains(&b))
        && !raw.starts_with([' ', '\t'])
        && !raw.ends_with([' ', '\t']);
    let sentinel_lookalike = raw.starts_with(B64_PREFIX) && raw.ends_with(B64_SUFFIX);

    if safe && !sentinel_lookalike {
        raw.to_owned()
    } else {
        format!("{B64_PREFIX}{}{B64_SUFFIX}", STANDARD.encode(raw))
    }
}

/// Reverses [`encode_header_value`], so a header can be compared to the body
/// value it mirrors.
///
/// Returns `None` when the sentinel is present but its payload is not valid
/// Base64-encoded UTF-8 -- the value claims an encoding it does not honor, and
/// the caller must reject rather than guess.
#[cfg(all(feature = "http-server", not(feature = "legacy-spec")))]
pub(crate) fn decode_header_value(value: &str) -> Option<String> {
    use base64::{Engine, engine::general_purpose::STANDARD};

    let Some(payload) = value
        .strip_prefix(B64_PREFIX)
        .and_then(|v| v.strip_suffix(B64_SUFFIX))
    else {
        return Some(value.to_owned());
    };
    STANDARD
        .decode(payload)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

/// Protocol-version routing header, required on every POST under
/// MCP 2026-07-28. Lets proxies route and lets the server reject
/// mismatched clients. Visible to both client (sends it) and server
/// (validates it), so it is not gated on `http-client`.
#[cfg(not(feature = "legacy-spec"))]
pub(crate) const MCP_PROTOCOL_VERSION: &str = "MCP-Protocol-Version";

/// `X-Content-Type-Options: nosniff`, sent on every SSE response.
///
/// A browser client reading the stream with `fetch()` is the case this exists
/// for: without the header Firefox buffers the body to sniff its type, and an
/// SSE stream that stays open never reaches the size that ends the sniff -- so
/// no event is delivered and the connection simply appears to hang. Chrome does
/// not sniff a declared `text/event-stream`, which is what makes the failure
/// look like a browser quirk rather than a missing header.
///
/// Non-browser consumers (neva's own client, an SDK proxy) never sniff, so this
/// costs them nothing. It is also the ordinary hardening answer for any
/// endpoint whose content type must be taken at its word.
#[cfg(feature = "http-server-volga")]
pub(crate) const CONTENT_TYPE_OPTIONS: (&str, &str) = ("X-Content-Type-Options", "nosniff");

const DEFAULT_ADDR: &str = "127.0.0.1:3000";
const DEFAULT_MCP_ENDPOINT: &str = "/mcp";

/// Default number of SSE events buffered per session for Last-Event-ID replay.
#[cfg(feature = "http-server")]
pub(crate) const DEFAULT_SSE_BUFFER_CAPACITY: usize = 64;
/// Default number of tracked SSE events queued for a live connection.
#[cfg(feature = "http-server")]
pub(crate) const DEFAULT_SSE_LIVE_QUEUE_CAPACITY: usize = 256;
/// Default number of ephemeral log events queued for a live connection.
#[cfg(feature = "http-server")]
pub(crate) const DEFAULT_SSE_LOG_QUEUE_CAPACITY: usize = 256;
/// Default interval between stale SSE session cleanup sweeps.
#[cfg(feature = "http-server")]
pub(crate) const DEFAULT_SSE_CLEANUP_INTERVAL: Duration = Duration::from_secs(300);
/// Default inactivity TTL for disconnected SSE sessions before eviction.
#[cfg(feature = "http-server")]
pub(crate) const DEFAULT_SSE_SESSION_TTL: Duration = Duration::from_secs(1800);

#[inline]
#[cfg(feature = "http-client")]
pub(super) fn get_mcp_session_id(headers: &HeaderMap) -> Option<uuid::Uuid> {
    headers
        .get(MCP_SESSION_ID)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
}

/// HTTP type
#[derive(Debug, Clone, Copy)]
pub(crate) enum HttpProto {
    Http,
    #[cfg(any(feature = "server-tls", feature = "client-tls"))]
    Https,
}

/// Streamable HTTP server transport.
///
/// Generic on a `Claims` type and an
/// [`HttpEngine`].
/// Under the default `http-server-volga` feature, type defaults make
/// `HttpServer::new(addr)` resolve to
/// `HttpServer<DefaultClaims, VolgaEngine>`; under `http-server-core`
/// alone, type params must be supplied (e.g. via
/// [`HttpServer::from_engine`](Self::from_engine)).
///
/// The engine is held in an `Option` purely so it can be `.take()`d out
/// of `&mut self` during `start()` (which moves the engine into a spawned
/// task). The `Option` is always `Some` between construction and the
/// first `start()` call.
#[cfg(feature = "http-server")]
pub struct HttpServer<C, E>
where
    E: HttpEngine,
{
    url: ServiceUrl,
    engine: Option<E>,
    sse_buffer_capacity: usize,
    sse_live_queue_capacity: usize,
    sse_log_queue_capacity: usize,
    sse_cleanup_interval: Duration,
    sse_session_ttl: Duration,
    /// `None` means "derive from the bind address" -- see
    /// [`Self::with_allowed_origins`].
    origin_policy: Option<core::origin::OriginPolicy>,
    #[cfg(feature = "server-oauth")]
    oauth: Option<core::oauth::OAuthResourceOptions>,
    sender: HttpSender,
    receiver: HttpReceiver,
    _claims: std::marker::PhantomData<fn() -> C>,
}

/// Streamable HTTP client transport.
#[cfg(feature = "http-client")]
pub struct HttpClient {
    url: ServiceUrl,
    access_token: Option<Box<[u8]>>,
    #[cfg(not(feature = "legacy-spec"))]
    peer_mode: crate::shared::PeerMode,
    #[cfg(all(not(feature = "legacy-spec"), feature = "http-client"))]
    param_headers: crate::shared::param_headers::Registry,
    #[cfg(feature = "client-oauth")]
    oauth: Option<client::oauth::OAuthClientConfig>,
    #[cfg(feature = "client-tls")]
    tls_config: Option<McpClientTlsConfig>,
    sender: HttpSender,
    receiver: HttpReceiver,
}

#[derive(Debug, Clone)]
pub(crate) struct ServiceUrl {
    proto: HttpProto,
    addr: String,
    endpoint: String,
}

#[cfg(feature = "http-client")]
pub(super) struct ClientRuntimeContext {
    url: ServiceUrl,
    tx: Sender<Result<Message, Error>>,
    rx: Receiver<Message>,
    access_token: Option<Box<[u8]>>,
    #[cfg(not(feature = "legacy-spec"))]
    pub(super) peer_mode: crate::shared::PeerMode,
    #[cfg(all(not(feature = "legacy-spec"), feature = "http-client"))]
    pub(super) param_headers: crate::shared::param_headers::Registry,
    #[cfg(feature = "client-oauth")]
    oauth: Option<std::sync::Arc<client::oauth::OAuthSession>>,
    #[cfg(feature = "client-tls")]
    tls_config: Option<ClientTlsConfig>,
}

/// Represents HTTP sender
pub(crate) struct HttpSender {
    tx: Sender<Message>,
    rx: Option<Receiver<Message>>,
}

/// Represents HTTP receiver
pub(crate) struct HttpReceiver {
    tx: Sender<Result<Message, Error>>,
    rx: Receiver<Result<Message, Error>>,
}

#[cfg(feature = "http-server")]
impl<C, E> std::fmt::Debug for HttpServer<C, E>
where
    E: HttpEngine + std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpServer")
            .field("url", &self.url)
            .field("engine", &self.engine)
            .field("sse_buffer_capacity", &self.sse_buffer_capacity)
            .field("sse_live_queue_capacity", &self.sse_live_queue_capacity)
            .field("sse_log_queue_capacity", &self.sse_log_queue_capacity)
            .field("sse_cleanup_interval", &self.sse_cleanup_interval)
            .field("sse_session_ttl", &self.sse_session_ttl)
            .finish()
    }
}

#[cfg(feature = "http-server-volga")]
impl Default for HttpServer<server::DefaultClaims, server::VolgaEngine> {
    #[inline]
    fn default() -> Self {
        Self {
            url: ServiceUrl::default(),
            engine: Some(VolgaEngine::default()),
            sse_buffer_capacity: DEFAULT_SSE_BUFFER_CAPACITY,
            sse_live_queue_capacity: DEFAULT_SSE_LIVE_QUEUE_CAPACITY,
            sse_log_queue_capacity: DEFAULT_SSE_LOG_QUEUE_CAPACITY,
            sse_cleanup_interval: DEFAULT_SSE_CLEANUP_INTERVAL,
            sse_session_ttl: DEFAULT_SSE_SESSION_TTL,
            origin_policy: None,
            #[cfg(feature = "server-oauth")]
            oauth: None,
            receiver: HttpReceiver::new(),
            sender: HttpSender::new(),
            _claims: std::marker::PhantomData,
        }
    }
}

#[cfg(feature = "http-client")]
impl Default for HttpClient {
    #[inline]
    fn default() -> Self {
        Self {
            url: ServiceUrl::default(),
            access_token: None,
            #[cfg(not(feature = "legacy-spec"))]
            peer_mode: Default::default(),
            #[cfg(all(not(feature = "legacy-spec"), feature = "http-client"))]
            param_headers: Default::default(),
            #[cfg(feature = "client-oauth")]
            oauth: None,
            #[cfg(feature = "client-tls")]
            tls_config: None,
            receiver: HttpReceiver::new(),
            sender: HttpSender::new(),
        }
    }
}

#[cfg(feature = "http-client")]
impl std::fmt::Debug for HttpClient {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpClient")
            .field("url", &self.url)
            .finish()
    }
}

#[cfg(feature = "http-client")]
impl ServiceUrl {
    /// Builds the full request URL (`proto://addr/endpoint`).
    ///
    /// Note: this **allocates** a fresh `String` -- it is not a cheap borrow
    /// despite reading stored fields. Assemble it once and cache the result
    /// (as `McpSession` does) rather than calling it per request.
    #[inline]
    pub(crate) fn to_url(&self) -> String {
        format!("{}://{}{}", self.proto, self.addr, self.endpoint)
    }
}

impl Display for HttpProto {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self {
            HttpProto::Http => f.write_str("http"),
            #[cfg(any(feature = "server-tls", feature = "client-tls"))]
            HttpProto::Https => f.write_str("https"),
        }
    }
}

impl Default for ServiceUrl {
    #[inline]
    fn default() -> Self {
        Self {
            proto: HttpProto::Http,
            addr: DEFAULT_ADDR.to_string(),
            endpoint: DEFAULT_MCP_ENDPOINT.to_string(),
        }
    }
}

impl Display for ServiceUrl {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://{}{}", self.proto, self.addr, self.endpoint)
    }
}

impl From<&str> for ServiceUrl {
    #[inline]
    fn from(url: &str) -> Self {
        let mut parts = MemChr::split(url, b'/');
        Self {
            proto: HttpProto::Http,
            addr: parts.nth(0).unwrap_or(DEFAULT_ADDR).to_string(),
            endpoint: parts.nth(1).unwrap_or(DEFAULT_MCP_ENDPOINT).to_string(),
        }
    }
}

impl Clone for HttpSender {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            rx: None,
        }
    }
}

impl HttpSender {
    /// Creates a new stdio transport sender
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::channel(100);
        Self { tx, rx: Some(rx) }
    }
}

impl HttpReceiver {
    /// Creates a new stdio transport receiver
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::channel(100);
        Self { tx, rx }
    }
}

#[cfg(feature = "http-server")]
impl<E> HttpServer<crate::auth::DefaultClaims, E>
where
    E: HttpEngine,
{
    /// Creates a new [`HttpServer`] bound to `addr`, running the supplied
    /// engine. This is the engine-agnostic constructor -- use it when
    /// plugging in a non-default engine.
    ///
    /// Returns `HttpServer<DefaultClaims, E>`. For a custom claims type,
    /// construct via the generic [`Self::with_engine`] swap on an existing
    /// server.
    ///
    /// # Example
    /// ```rust,ignore
    /// let server = HttpServer::from_engine("127.0.0.1:3000", MyAxumEngine::new());
    /// ```
    pub fn from_engine(addr: impl AsRef<str>, engine: E) -> Self {
        let url = ServiceUrl {
            addr: addr.as_ref().to_owned(),
            ..ServiceUrl::default()
        };
        Self {
            url,
            engine: Some(engine),
            sse_buffer_capacity: DEFAULT_SSE_BUFFER_CAPACITY,
            sse_live_queue_capacity: DEFAULT_SSE_LIVE_QUEUE_CAPACITY,
            sse_log_queue_capacity: DEFAULT_SSE_LOG_QUEUE_CAPACITY,
            sse_cleanup_interval: DEFAULT_SSE_CLEANUP_INTERVAL,
            sse_session_ttl: DEFAULT_SSE_SESSION_TTL,
            origin_policy: None,
            #[cfg(feature = "server-oauth")]
            oauth: None,
            receiver: HttpReceiver::new(),
            sender: HttpSender::new(),
            _claims: std::marker::PhantomData,
        }
    }
}

#[cfg(feature = "http-server")]
impl<C, E> HttpServer<C, E>
where
    E: HttpEngine,
{
    /// Binds HTTP serve to address and port
    pub fn bind(mut self, addr: impl AsRef<str>) -> Self {
        self.url.addr = addr.as_ref().to_owned();
        self
    }

    /// Sets the MCP endpoint
    ///
    /// Default: `/mcp`
    pub fn with_endpoint(mut self, prefix: impl AsRef<str>) -> Self {
        self.url.endpoint = prefix.as_ref().to_owned();
        self
    }

    /// Names the hosts this server answers to, in addition to the loopback
    /// ones, and turns on `Origin` / `Host` checking if the bind address did
    /// not already.
    ///
    /// # Why this exists
    ///
    /// A server on loopback is reachable by any page the browser loads: point
    /// `evil.example.com` at `127.0.0.1` and the browser will connect. The
    /// request is genuinely local; what gives the attack away is the name it
    /// was addressed by. The spec therefore requires local servers to validate
    /// these headers and answer `403 Forbidden` when they do not check out.
    ///
    /// # The default needs no call
    ///
    /// Bound to loopback, a server already accepts only loopback names --
    /// `localhost`, anything in `127.0.0.0/8`, `[::1]` -- on any port. Bound to
    /// anything else it accepts everything, because the names a deployment is
    /// legitimately reached by are not knowable from here: behind a proxy the
    /// `Host` is whatever that proxy forwards. This method is how such a
    /// deployment states them.
    ///
    /// Entries are hostnames; a port in one is ignored, since the port a
    /// request arrives on says nothing about who sent it. Matching is
    /// case-insensitive. A request carrying neither header is not from a
    /// browser and is left alone -- there is no rebinding without a name.
    ///
    /// # Example
    /// ```rust,ignore
    /// use neva::transport::http::HttpServer;
    ///
    /// let server = HttpServer::new("0.0.0.0:3000")
    ///     .with_allowed_origins(["mcp.example.com", "app.example.com"]);
    /// ```
    pub fn with_allowed_origins<I, S>(mut self, hosts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let hosts = hosts
            .into_iter()
            .map(|h| Box::from(h.as_ref()))
            .collect::<Vec<Box<str>>>();
        self.origin_policy = Some(core::origin::OriginPolicy::Allowlist(hosts.into()));
        self
    }

    /// Answers to any `Origin` and `Host`, turning off the DNS-rebinding gate.
    ///
    /// Only meaningful on a loopback bind, where the gate is on by default.
    /// Reach for this when something in front of the server already validates
    /// the name -- not to quiet a rejection whose cause has not been read,
    /// since that rejection is the protection working.
    ///
    /// # Example
    /// ```rust,ignore
    /// use neva::transport::http::HttpServer;
    ///
    /// // A tunnel terminates the browser-facing name and forwards here.
    /// let server = HttpServer::new("127.0.0.1:3000").allow_any_origin();
    /// ```
    pub fn allow_any_origin(mut self) -> Self {
        self.origin_policy = Some(core::origin::OriginPolicy::Any);
        self
    }

    /// Swap the HTTP engine. Engine-specific config (auth, TLS) does not
    /// carry over -- the new engine starts with its own defaults.
    pub fn with_engine<E2>(self, engine: E2) -> HttpServer<C, E2>
    where
        E2: HttpEngine,
    {
        HttpServer {
            url: self.url,
            engine: Some(engine),
            sse_buffer_capacity: self.sse_buffer_capacity,
            sse_live_queue_capacity: self.sse_live_queue_capacity,
            sse_log_queue_capacity: self.sse_log_queue_capacity,
            sse_cleanup_interval: self.sse_cleanup_interval,
            sse_session_ttl: self.sse_session_ttl,
            // Carried across the swap: the DNS-rebinding gate is a property of
            // the deployment, not of which engine serves it.
            origin_policy: self.origin_policy,
            #[cfg(feature = "server-oauth")]
            oauth: self.oauth,
            sender: self.sender,
            receiver: self.receiver,
            _claims: std::marker::PhantomData,
        }
    }

    /// Sets the SSE event buffer capacity per session for Last-Event-ID replay.
    ///
    /// Defaults to `64`. Pass `0` to disable buffering.
    ///
    /// # Example
    /// ```rust,ignore
    /// HttpServer::new("127.0.0.1:3000")
    ///     .with_endpoint("/mcp")
    ///     .with_sse_buffer(256)
    /// ```
    pub fn with_sse_buffer(mut self, capacity: usize) -> Self {
        self.sse_buffer_capacity = capacity;
        self
    }

    /// Sets the live SSE queue capacity per active connection for tracked MCP events.
    ///
    /// Defaults to `256`.
    /// When the queue fills, the live connection is disconnected and recent
    /// events remain available through the replay buffer configured by
    /// [`HttpServer::with_sse_buffer`].
    ///
    /// # Example
    /// ```rust,ignore
    /// HttpServer::new("127.0.0.1:3000")
    ///     .with_sse_live_queue(512)
    /// ```
    pub fn with_sse_live_queue(mut self, capacity: usize) -> Self {
        assert!(
            capacity > 0,
            "SSE live queue capacity must be greater than 0"
        );
        self.sse_live_queue_capacity = capacity;
        self
    }

    /// Sets the live SSE queue capacity per active connection for ephemeral log events.
    ///
    /// Defaults to `256`.
    /// When the queue fills, new log notifications are dropped.
    ///
    /// # Example
    /// ```rust,ignore
    /// HttpServer::new("127.0.0.1:3000")
    ///     .with_sse_log_queue(128)
    /// ```
    pub fn with_sse_log_queue(mut self, capacity: usize) -> Self {
        assert!(
            capacity > 0,
            "SSE log queue capacity must be greater than 0"
        );
        self.sse_log_queue_capacity = capacity;
        self
    }

    /// Sets how often stale SSE sessions are scanned for eviction.
    ///
    /// Defaults to `300s`.
    ///
    /// # Example
    /// ```rust,ignore
    /// use std::time::Duration;
    ///
    /// HttpServer::new("127.0.0.1:3000")
    ///     .with_sse_cleanup_interval(Duration::from_secs(60))
    /// ```
    pub fn with_sse_cleanup_interval(mut self, interval: Duration) -> Self {
        assert!(
            !interval.is_zero(),
            "SSE cleanup interval must be greater than 0"
        );
        self.sse_cleanup_interval = interval;
        self
    }

    /// Sets the inactivity TTL for disconnected SSE sessions before eviction.
    ///
    /// Defaults to `1800s`.
    ///
    /// # Example
    /// ```rust,ignore
    /// use std::time::Duration;
    ///
    /// HttpServer::new("127.0.0.1:3000")
    ///     .with_sse_session_ttl(Duration::from_secs(7200))
    /// ```
    pub fn with_sse_session_ttl(mut self, ttl: Duration) -> Self {
        assert!(!ttl.is_zero(), "SSE session TTL must be greater than 0");
        self.sse_session_ttl = ttl;
        self
    }

    /// Configures the OAuth Protected Resource Metadata document
    /// (RFC 9728) advertised by this server. Engine-neutral: the resolved
    /// document reaches the engine through
    /// [`HttpContext::oauth_metadata_path`] and the
    /// [`handlers::handle_oauth_metadata`] /
    /// [`handlers::handle_unauthorized`] helpers.
    ///
    /// The resource identifier defaults to this server's own URL; override
    /// it with
    /// [`OAuthResourceOptions::with_resource`](core::oauth::OAuthResourceOptions::with_resource)
    /// when running behind a reverse proxy.
    ///
    /// # Example
    /// ```rust,ignore
    /// HttpServer::new("127.0.0.1:3000")
    ///     .with_oauth_metadata(|oauth| oauth
    ///         .with_authorization_servers(["https://auth.example.com"])
    ///         .with_scopes(["mcp:tools"]))
    /// ```
    #[cfg(feature = "server-oauth")]
    pub fn with_oauth_metadata<F>(mut self, config: F) -> Self
    where
        F: FnOnce(core::oauth::OAuthResourceOptions) -> core::oauth::OAuthResourceOptions,
    {
        self.oauth = Some(config(core::oauth::OAuthResourceOptions::default()));
        self
    }

    fn build_context_and_engine(&mut self) -> Result<(HttpContext, Receiver<Message>), Error> {
        // Resolve the OAuth resource once at startup: canonicalize the
        // identifier, pre-serialize the RFC 9728 document, pre-render the
        // challenge. A misconfigured resource URI fails server start.
        // Resolved from a clone, before any `self` state is consumed, so
        // a config failure leaves the transport untouched.
        #[cfg(feature = "server-oauth")]
        let oauth = self
            .oauth
            .clone()
            .map(|o| o.resolve(&self.url.to_string()))
            .transpose()?;
        let Some(sender_rx) = self.sender.rx.take() else {
            return Err(Error::new(
                ErrorCode::InternalError,
                "The HTTP writer is already in use",
            ));
        };
        let pending = std::sync::Arc::new(dashmap::DashMap::new());
        let sse_registry = std::sync::Arc::new(crate::shared::SseSessionRegistry::new(
            self.sse_buffer_capacity,
        ));
        let ctx = HttpContext {
            addr: self.url.addr.as_str().into(),
            endpoint: self.url.endpoint.as_str().into(),
            pending,
            sse_registry,
            inbound_tx: self.receiver.tx.clone(),
            sse_live_queue_capacity: self.sse_live_queue_capacity,
            sse_log_queue_capacity: self.sse_log_queue_capacity,
            origin_policy: self
                .origin_policy
                .clone()
                .unwrap_or_else(|| core::origin::OriginPolicy::for_addr(&self.url.addr)),
            #[cfg(feature = "server-oauth")]
            oauth,
        };
        Ok((ctx, sender_rx))
    }
}

#[cfg(feature = "http-server-volga")]
impl HttpServer<server::DefaultClaims, VolgaEngine> {
    /// Creates a new `HttpServer` bound to the given address, using the
    /// default Volga engine.
    ///
    /// # Example
    /// ```rust,ignore
    /// use neva::transport::http::HttpServer;
    ///
    /// let _ = HttpServer::new("127.0.0.1:3000");
    /// ```
    pub fn new(addr: impl AsRef<str>) -> Self {
        let url = ServiceUrl {
            addr: addr.as_ref().to_owned(),
            ..ServiceUrl::default()
        };
        Self {
            url,
            engine: Some(VolgaEngine::default()),
            sse_buffer_capacity: DEFAULT_SSE_BUFFER_CAPACITY,
            sse_live_queue_capacity: DEFAULT_SSE_LIVE_QUEUE_CAPACITY,
            sse_log_queue_capacity: DEFAULT_SSE_LOG_QUEUE_CAPACITY,
            sse_cleanup_interval: DEFAULT_SSE_CLEANUP_INTERVAL,
            sse_session_ttl: DEFAULT_SSE_SESSION_TTL,
            origin_policy: None,
            #[cfg(feature = "server-oauth")]
            oauth: None,
            receiver: HttpReceiver::new(),
            sender: HttpSender::new(),
            _claims: std::marker::PhantomData,
        }
    }

    /// Configures authentication and authorization (Volga-specific).
    pub fn with_auth<F>(mut self, config: F) -> Self
    where
        F: FnOnce(server::AuthConfig) -> server::AuthConfig,
    {
        let auth = config(server::AuthConfig::default());
        // Default-flow glue: when OAuth issuer mode is on and no
        // Protected Resource Metadata was configured explicitly, derive
        // the document from that issuer -- the well-known route and the
        // 401 challenge then work out of the box. An explicit
        // `with_oauth_metadata` (before or after this call) wins.
        #[cfg(feature = "server-oauth")]
        if self.oauth.is_none()
            && let Some(issuer) = auth.oauth_issuer()
        {
            self.oauth = Some(
                core::oauth::OAuthResourceOptions::default().with_authorization_servers([issuer]),
            );
        }
        let engine = self
            .engine
            .as_mut()
            .expect("HttpServer::with_auth called after start()");
        engine.auth = Some(auth);
        self
    }

    /// Configures TLS (Volga-specific).
    #[cfg(feature = "server-tls")]
    pub fn with_tls<F>(mut self, config: F) -> Self
    where
        F: FnOnce(TlsConfig) -> TlsConfig,
    {
        let engine = self
            .engine
            .as_mut()
            .expect("HttpServer::with_tls called after start()");
        engine.tls = Some(config(Default::default()));
        self.url.proto = HttpProto::Https;
        self
    }
}

#[cfg(feature = "http-client")]
impl HttpClient {
    /// Binds HTTP serve to address and port    
    pub fn bind(mut self, addr: impl AsRef<str>) -> Self {
        self.url.addr = addr.as_ref().to_owned();
        self
    }

    /// Sets the MCP endpoint
    ///
    /// Default: `/mcp`
    pub fn with_endpoint(mut self, prefix: impl AsRef<str>) -> Self {
        self.url.endpoint = prefix.as_ref().to_owned();
        self
    }

    /// Sets the TLS config for this MCP client
    #[cfg(feature = "client-tls")]
    pub fn with_tls<F>(mut self, config: F) -> Self
    where
        F: FnOnce(McpClientTlsConfig) -> McpClientTlsConfig,
    {
        self.tls_config = Some(config(Default::default()));
        self.url.proto = HttpProto::Https;
        self
    }

    /// Hands the `x-mcp-header` registry to this transport, so a `tools/call`
    /// can mirror the designated arguments into `Mcp-Param-*` headers.
    #[cfg(all(not(feature = "legacy-spec"), feature = "http-client"))]
    pub(crate) fn with_param_headers(
        mut self,
        registry: crate::shared::param_headers::Registry,
    ) -> Self {
        self.param_headers = registry;
        self
    }

    /// Hands the dual-mode protocol switch to this transport (set by
    /// `McpOptions::transport`) so per-request headers follow the
    /// negotiated protocol generation.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) fn with_peer_mode(mut self, peer_mode: crate::shared::PeerMode) -> Self {
        self.peer_mode = peer_mode;
        self
    }

    /// Set the bearer token for requests
    ///
    ///Default: `None`
    pub fn with_auth(mut self, access_token: impl Into<String>) -> Self {
        self.access_token = Some(access_token.into().into_bytes().into_boxed_slice());
        self
    }

    /// Enables automatic OAuth 2.1 authorization: on a `401` challenge
    /// the client discovers the server's authorization requirements,
    /// registers itself when needed, runs the authorization-code + PKCE
    /// flow and attaches the obtained token to every request.
    ///
    /// Takes precedence over a static [`with_auth`](Self::with_auth)
    /// token.
    ///
    /// # Example
    /// ```no_run
    /// use neva::Client;
    ///
    /// let mut client = Client::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_oauth(|oauth| oauth.with_scopes(["mcp:tools"]))
    ///         )
    ///     );
    /// ```
    #[cfg(feature = "client-oauth")]
    pub fn with_oauth<F>(mut self, config: F) -> Self
    where
        F: FnOnce(client::oauth::OAuthClientConfig) -> client::oauth::OAuthClientConfig,
    {
        self.oauth = Some(config(client::oauth::OAuthClientConfig::default()));
        self
    }

    fn runtime(&mut self) -> Result<ClientRuntimeContext, Error> {
        // Build the OAuth session before consuming transport state --
        // same rationale as the server-side OAuth resolve.
        #[cfg(feature = "client-oauth")]
        let oauth = self
            .oauth
            .take()
            .map(|config| client::oauth::OAuthSession::new(config, &self.url.to_url()))
            .transpose()?
            .map(std::sync::Arc::new);

        let Some(sender_rx) = self.sender.rx.take() else {
            return Err(Error::new(
                ErrorCode::InternalError,
                "The HTTP writer is already in use",
            ));
        };

        #[cfg(feature = "client-tls")]
        let tls_config = self.tls_config.take().map(|tls| tls.build()).transpose()?;

        Ok(ClientRuntimeContext {
            url: self.url.clone(),
            tx: self.receiver.tx.clone(),
            rx: sender_rx,
            access_token: self.access_token.take(),
            #[cfg(not(feature = "legacy-spec"))]
            peer_mode: self.peer_mode.clone(),
            #[cfg(all(not(feature = "legacy-spec"), feature = "http-client"))]
            param_headers: self.param_headers.clone(),
            #[cfg(feature = "client-oauth")]
            oauth,
            #[cfg(feature = "client-tls")]
            tls_config,
        })
    }
}

impl TransportSender for HttpSender {
    async fn send(&mut self, msg: Message) -> Result<(), Error> {
        self.tx
            .send(msg)
            .map_err(|err| Error::new(ErrorCode::InternalError, err))
            .await
    }
}

impl TransportReceiver for HttpReceiver {
    async fn recv(&mut self) -> Result<Message, Error> {
        self.rx.recv().await.unwrap_or_else(|| {
            Err(Error::new(
                ErrorCode::InvalidRequest,
                "Unexpected end of stream",
            ))
        })
    }
}

#[cfg(feature = "http-server")]
impl<C, E> Transport for HttpServer<C, E>
where
    C: Send + 'static,
    E: HttpEngine,
{
    type Sender = HttpSender;
    type Receiver = HttpReceiver;

    fn start(&mut self) -> CancellationToken {
        let token = CancellationToken::new();
        let (ctx, sender_rx) = match self.build_context_and_engine() {
            Ok(x) => x,
            Err(_err) => {
                #[cfg(feature = "tracing")]
                tracing::error!(logger = "neva", "Failed to start HTTP server: {}", _err);
                // Hand back an already-cancelled token so `App::run`'s
                // receive loop breaks immediately instead of waiting
                // forever on a server that never bound.
                token.cancel();
                return token;
            }
        };

        // Take the engine out of the Option so we can move it into the
        // spawned task. start() must only be called once per HttpServer
        // -- the App's run loop owns the HttpServer instance and calls
        // start() exactly once.
        let engine = self
            .engine
            .take()
            .expect("HttpServer::start called twice or after engine was moved");

        let pending = ctx.pending.clone();
        let sse_registry = ctx.sse_registry.clone();
        let cleanup_registry = ctx.sse_registry.clone();
        let cleanup_interval = self.sse_cleanup_interval;
        let session_ttl = self.sse_session_ttl;
        let engine_token = token.clone();

        tokio::spawn(async move {
            tokio::join!(
                core::dispatch::dispatch(pending, sse_registry, sender_rx, engine_token.clone(),),
                core::cleanup::cleanup_stale_sessions(
                    cleanup_registry,
                    cleanup_interval,
                    session_ttl,
                    engine_token.clone(),
                ),
                async {
                    if let Err(_e) = engine.run(ctx, engine_token.clone()).await {
                        #[cfg(feature = "tracing")]
                        tracing::error!(logger = "neva", "HTTP engine error: {:?}", _e);
                        engine_token.cancel();
                    }
                }
            );
        });

        token
    }

    #[inline]
    fn split(self) -> (Self::Sender, Self::Receiver) {
        (self.sender, self.receiver)
    }
}

#[cfg(feature = "http-server")]
impl<C, E> core::engine::HttpTransport for HttpServer<C, E>
where
    C: Send + 'static,
    E: HttpEngine,
{
    fn start(&mut self) -> CancellationToken {
        <Self as Transport>::start(self)
    }

    fn split_into_proto(self: Box<Self>) -> (HttpSender, HttpReceiver) {
        let s = *self;
        Transport::split(s)
    }

    fn url_label(&self) -> String {
        self.url.to_string()
    }
}

#[cfg(feature = "http-client")]
impl Transport for HttpClient {
    type Sender = HttpSender;
    type Receiver = HttpReceiver;

    fn start(&mut self) -> CancellationToken {
        let token = CancellationToken::new();
        let runtime = match self.runtime() {
            Ok(runtime) => runtime,
            Err(_err) => {
                #[cfg(feature = "tracing")]
                tracing::error!(logger = "neva", "Failed to start HTTP client: {}", _err);
                return token;
            }
        };
        tokio::spawn(client::connect(runtime, token.clone()));

        token
    }

    fn split(self) -> (Self::Sender, Self::Receiver) {
        (self.sender, self.receiver)
    }
}

#[cfg(all(test, feature = "http-server"))]
mod engine_smoke_tests {
    use super::*;
    use crate::error::Error;
    use crate::transport::Transport;
    use crate::transport::http::core::{
        context::HttpContext,
        engine::HttpEngine,
        types::{HttpRequest, HttpResponse},
    };
    use crate::types::Message;
    use std::future::Future;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[derive(Default)]
    struct MockEngine {
        started: Arc<AtomicBool>,
        exited: Arc<AtomicBool>,
    }

    impl HttpEngine for MockEngine {
        type Request = HttpRequest;
        type Response = HttpResponse;
        type SseEvent = ();

        async fn adapt_request(req: Self::Request) -> Result<HttpRequest, Error> {
            Ok(req)
        }

        fn adapt_response(resp: HttpResponse) -> Self::Response {
            resp
        }

        fn tracked_event(_seq: u64, _msg: &Message) -> Self::SseEvent {}
        fn ephemeral_event(_msg: &Message) -> Self::SseEvent {}

        fn run(
            self,
            _ctx: HttpContext,
            token: CancellationToken,
        ) -> impl Future<Output = Result<(), Error>> + Send {
            let started = self.started;
            let exited = self.exited;
            async move {
                started.store(true, Ordering::SeqCst);
                token.cancelled().await;
                exited.store(true, Ordering::SeqCst);
                Ok(())
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn engine_run_is_invoked_and_cancellation_propagates() {
        let started = Arc::new(AtomicBool::new(false));
        let exited = Arc::new(AtomicBool::new(false));
        let engine = MockEngine {
            started: started.clone(),
            exited: exited.clone(),
        };
        let mut server = HttpServer::from_engine("127.0.0.1:0", engine);
        let token = <HttpServer<_, _> as Transport>::start(&mut server);

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(started.load(Ordering::SeqCst), "engine.run was not invoked");

        token.cancel();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            exited.load(Ordering::SeqCst),
            "engine did not exit on cancellation"
        );
    }

    #[cfg(feature = "server-oauth")]
    #[test]
    fn oauth_metadata_options_reach_the_engine_context() {
        let mut server = HttpServer::from_engine("127.0.0.1:3000", MockEngine::default())
            .with_oauth_metadata(|oauth| {
                oauth.with_authorization_servers(["https://auth.example.com"])
            });

        let (ctx, _rx) = server.build_context_and_engine().unwrap();

        assert_eq!(
            ctx.oauth_metadata_path(),
            Some("/.well-known/oauth-protected-resource/mcp")
        );
    }

    #[cfg(feature = "server-oauth")]
    #[test]
    fn invalid_oauth_resource_fails_server_start() {
        let mut server = HttpServer::from_engine("127.0.0.1:3000", MockEngine::default())
            .with_oauth_metadata(|oauth| oauth.with_resource("not a uri"));

        assert!(server.build_context_and_engine().is_err());
        // The config failure must not consume the HTTP writer -- it fires
        // before any transport state is taken.
        assert!(server.sender.rx.is_some());
    }

    #[cfg(all(feature = "http-server-volga", feature = "server-oauth"))]
    #[test]
    fn oauth_issuer_seeds_resource_metadata() {
        let mut server = HttpServer::new("127.0.0.1:3000").with_auth(|auth| {
            auth.with_oauth(|oauth| oauth.with_issuer("https://auth.example.com"))
        });

        let (ctx, _rx) = server.build_context_and_engine().unwrap();

        assert_eq!(
            ctx.oauth_metadata_path(),
            Some("/.well-known/oauth-protected-resource/mcp")
        );
        let resp = core::handlers::handle_oauth_metadata(&ctx);
        let doc: serde_json::Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(doc["authorization_servers"][0], "https://auth.example.com");
    }

    #[cfg(all(feature = "http-server-volga", feature = "server-oauth"))]
    #[test]
    fn explicit_metadata_wins_over_issuer_seeding() {
        let mut server = HttpServer::new("127.0.0.1:3000")
            .with_auth(|auth| {
                auth.with_oauth(|oauth| oauth.with_issuer("https://auth.example.com"))
            })
            .with_oauth_metadata(|oauth| {
                oauth.with_authorization_servers(["https://other.example.com"])
            });

        let (ctx, _rx) = server.build_context_and_engine().unwrap();

        let resp = core::handlers::handle_oauth_metadata(&ctx);
        let doc: serde_json::Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(doc["authorization_servers"][0], "https://other.example.com");
    }

    #[cfg(feature = "server-oauth")]
    #[tokio::test]
    async fn invalid_oauth_resource_cancels_startup_token() {
        let mut server = HttpServer::from_engine("127.0.0.1:3000", MockEngine::default())
            .with_oauth_metadata(|oauth| oauth.with_resource("not a uri"));

        let token = <HttpServer<_, _> as Transport>::start(&mut server);

        // A cancelled token breaks App::run's receive loop immediately;
        // an uncancelled one would leave the app waiting forever on a
        // server that never bound.
        assert!(token.is_cancelled());
    }
}

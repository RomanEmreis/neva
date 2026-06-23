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
    types::{HttpRequest, HttpResponse, SseResponse},
};

#[cfg(feature = "http-client")]
pub(crate) mod client;
#[cfg(feature = "http-server")]
pub mod core;
#[cfg(feature = "http-server-volga")]
pub(crate) mod server;

#[cfg(feature = "http-client")]
pub(super) const MCP_SESSION_ID: &str = "Mcp-Session-Id";

/// JSON-RPC method name carried on every outbound HTTP request under
/// `proto-2026-07-28-rc`. Allows reverse proxies and load balancers to
/// route without parsing the request body.
#[cfg(all(feature = "http-client", feature = "proto-2026-07-28-rc"))]
pub(super) const MCP_METHOD: &str = "Mcp-Method";

/// Entity name (today: tool name for `tools/call`) carried on every
/// outbound HTTP request under `proto-2026-07-28-rc`.
#[cfg(all(feature = "http-client", feature = "proto-2026-07-28-rc"))]
pub(super) const MCP_NAME: &str = "Mcp-Name";

/// Protocol-version routing header, required on every POST under
/// `proto-2026-07-28-rc`. Lets proxies route and lets the server reject
/// mismatched clients. Visible to both client (sends it) and server
/// (validates it), so it is not gated on `http-client`.
#[cfg(feature = "proto-2026-07-28-rc")]
pub(crate) const MCP_PROTOCOL_VERSION: &str = "MCP-Protocol-Version";

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
    sender: HttpSender,
    receiver: HttpReceiver,
    _claims: std::marker::PhantomData<fn() -> C>,
}

/// Streamable HTTP client transport.
#[cfg(feature = "http-client")]
pub struct HttpClient {
    url: ServiceUrl,
    access_token: Option<Box<[u8]>>,
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
    /// Note: this **allocates** a fresh `String` — it is not a cheap borrow
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
    /// engine. This is the engine-agnostic constructor — use it when
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

    /// Swap the HTTP engine. Engine-specific config (auth, TLS) does not
    /// carry over — the new engine starts with its own defaults.
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

    fn build_context_and_engine(&mut self) -> Result<(HttpContext, Receiver<Message>), Error> {
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
        let engine = self
            .engine
            .as_mut()
            .expect("HttpServer::with_auth called after start()");
        engine.auth = Some(config(server::AuthConfig::default()));
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

    /// Set the bearer token for requests
    ///
    ///Default: `None`
    pub fn with_auth(mut self, access_token: impl Into<String>) -> Self {
        self.access_token = Some(access_token.into().into_bytes().into_boxed_slice());
        self
    }

    fn runtime(&mut self) -> Result<ClientRuntimeContext, Error> {
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
                return token;
            }
        };

        // Take the engine out of the Option so we can move it into the
        // spawned task. start() must only be called once per HttpServer
        // — the App's run loop owns the HttpServer instance and calls
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
}

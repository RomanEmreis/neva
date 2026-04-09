//! Streamable HTTP transport implementation

#[cfg(all(feature = "client", not(feature = "server")))]
use reqwest::header::HeaderMap;

#[cfg(feature = "http-server")]
use {
    server::{AuthConfig, DefaultClaims},
    volga::{auth::AuthClaims, headers::HeaderMap},
};

use crate::{
    error::{Error, ErrorCode},
    shared::MemChr,
    types::Message,
};
use futures_util::TryFutureExt;
use std::{borrow::Cow, fmt::Display, time::Duration};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio_util::sync::CancellationToken;

use super::{Receiver as TransportReceiver, Sender as TransportSender, Transport};

#[cfg(all(feature = "http-server", feature = "server-tls"))]
pub use volga::tls::{DevCertMode, TlsConfig};

#[cfg(all(feature = "http-client", feature = "client-tls"))]
use crate::transport::http::client::tls_config::{
    ClientTlsConfig, TlsConfig as McpClientTlsConfig,
};

#[cfg(feature = "http-client")]
pub(crate) mod client;
#[cfg(feature = "http-server")]
pub(crate) mod server;

pub(super) const MCP_SESSION_ID: &str = "Mcp-Session-Id";
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

/// Represents HTTP server transport
#[cfg(feature = "http-server")]
pub struct HttpServer<C: AuthClaims = DefaultClaims> {
    url: ServiceUrl,
    auth: Option<AuthConfig<C>>,
    #[cfg(feature = "server-tls")]
    tls_config: Option<TlsConfig>,
    sse_buffer_capacity: usize,
    sse_live_queue_capacity: usize,
    sse_log_queue_capacity: usize,
    sse_cleanup_interval: Duration,
    sse_session_ttl: Duration,
    sender: HttpSender,
    receiver: HttpReceiver,
}

#[cfg(feature = "http-client")]
pub struct HttpClient {
    url: ServiceUrl,
    access_token: Option<Box<[u8]>>,
    #[cfg(feature = "client-tls")]
    tls_config: Option<McpClientTlsConfig>,
    sender: HttpSender,
    receiver: HttpReceiver,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ServiceUrl {
    proto: HttpProto,
    addr: &'static str,
    endpoint: &'static str,
}

#[cfg(feature = "http-server")]
pub(super) struct HttpRuntimeContext {
    url: ServiceUrl,
    tx: Sender<Result<Message, Error>>,
    #[cfg(feature = "server-tls")]
    tls_config: Option<TlsConfig>,
    rx: Receiver<Message>,
    auth: Option<AuthConfig>,
    pub(super) sse_buffer_capacity: usize,
    pub(super) sse_live_queue_capacity: usize,
    pub(super) sse_log_queue_capacity: usize,
    pub(super) sse_cleanup_interval: Duration,
    pub(super) sse_session_ttl: Duration,
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
impl std::fmt::Debug for HttpServer {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpServer")
            .field("url", &self.url)
            .field("sse_buffer_capacity", &self.sse_buffer_capacity)
            .field("sse_live_queue_capacity", &self.sse_live_queue_capacity)
            .field("sse_log_queue_capacity", &self.sse_log_queue_capacity)
            .field("sse_cleanup_interval", &self.sse_cleanup_interval)
            .field("sse_session_ttl", &self.sse_session_ttl)
            .finish()
    }
}

#[cfg(feature = "http-server")]
impl Default for HttpServer {
    #[inline]
    fn default() -> Self {
        Self {
            url: ServiceUrl::default(),
            auth: None,
            #[cfg(feature = "server-tls")]
            tls_config: None,
            sse_buffer_capacity: DEFAULT_SSE_BUFFER_CAPACITY,
            sse_live_queue_capacity: DEFAULT_SSE_LIVE_QUEUE_CAPACITY,
            sse_log_queue_capacity: DEFAULT_SSE_LOG_QUEUE_CAPACITY,
            sse_cleanup_interval: DEFAULT_SSE_CLEANUP_INTERVAL,
            sse_session_ttl: DEFAULT_SSE_SESSION_TTL,
            receiver: HttpReceiver::new(),
            sender: HttpSender::new(),
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

impl ServiceUrl {
    #[inline]
    pub(crate) fn as_str<'a>(&self) -> Cow<'a, str> {
        Cow::Owned(format!("{}://{}{}", self.proto, self.addr, self.endpoint))
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
            addr: DEFAULT_ADDR,
            endpoint: DEFAULT_MCP_ENDPOINT,
        }
    }
}

impl Display for ServiceUrl {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_str().fmt(f)
    }
}

impl From<&'static str> for ServiceUrl {
    #[inline]
    fn from(url: &'static str) -> Self {
        let mut parts = MemChr::split(url, b'/');
        Self {
            proto: HttpProto::Http,
            addr: parts.nth(0).unwrap_or(DEFAULT_ADDR),
            endpoint: parts.nth(1).unwrap_or(DEFAULT_MCP_ENDPOINT),
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
impl HttpServer {
    /// Creates a new [`HttpServer`]
    #[inline]
    pub fn new(addr: &'static str) -> Self {
        Self::default().bind(addr)
    }

    /// Binds HTTP serve to address and port    
    pub fn bind(mut self, addr: &'static str) -> Self {
        self.url.addr = addr;
        self
    }

    /// Sets the MCP endpoint
    ///
    /// Default: `/mcp`
    pub fn with_endpoint(mut self, prefix: &'static str) -> Self {
        self.url.endpoint = prefix;
        self
    }

    /// Configures HTTP server's TLS configuration
    #[cfg(feature = "server-tls")]
    pub fn with_tls<F>(mut self, config: F) -> Self
    where
        F: FnOnce(TlsConfig) -> TlsConfig,
    {
        self.tls_config = Some(config(Default::default()));
        self.url.proto = HttpProto::Https;
        self
    }

    /// Configures authentication and authorization
    pub fn with_auth<F>(mut self, config: F) -> Self
    where
        F: FnOnce(AuthConfig) -> AuthConfig,
    {
        self.auth = Some(config(AuthConfig::default()));
        self
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

    /// Returns the URL label used for display in the greeting banner
    pub(crate) fn url_label(&self) -> String {
        self.url.to_string()
    }

    fn runtime(&mut self) -> Result<HttpRuntimeContext, Error> {
        let Some(sender_rx) = self.sender.rx.take() else {
            return Err(Error::new(
                ErrorCode::InternalError,
                "The HTTP writer is already in use",
            ));
        };
        Ok(HttpRuntimeContext {
            url: self.url,
            tx: self.receiver.tx.clone(),
            rx: sender_rx,
            auth: self.auth.take(),
            #[cfg(feature = "server-tls")]
            tls_config: self.tls_config.take(),
            sse_buffer_capacity: self.sse_buffer_capacity,
            sse_live_queue_capacity: self.sse_live_queue_capacity,
            sse_log_queue_capacity: self.sse_log_queue_capacity,
            sse_cleanup_interval: self.sse_cleanup_interval,
            sse_session_ttl: self.sse_session_ttl,
        })
    }
}

#[cfg(feature = "http-client")]
impl HttpClient {
    /// Binds HTTP serve to address and port    
    pub fn bind(mut self, addr: &'static str) -> Self {
        self.url.addr = addr;
        self
    }

    /// Sets the MCP endpoint
    ///
    /// Default: `/mcp`
    pub fn with_endpoint(mut self, prefix: &'static str) -> Self {
        self.url.endpoint = prefix;
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
            url: self.url,
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
impl Transport for HttpServer {
    type Sender = HttpSender;
    type Receiver = HttpReceiver;

    fn start(&mut self) -> CancellationToken {
        let token = CancellationToken::new();
        let runtime = match self.runtime() {
            Ok(runtime) => runtime,
            Err(_err) => {
                #[cfg(feature = "tracing")]
                tracing::error!(logger = "neva", "Failed to start HTTP server: {}", _err);
                return token;
            }
        };
        tokio::spawn(server::serve(runtime, token.clone()));

        token
    }

    #[inline]
    fn split(self) -> (Self::Sender, Self::Receiver) {
        (self.sender, self.receiver)
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

#[cfg(test)]
mod test {}

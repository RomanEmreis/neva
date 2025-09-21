//! Streamable HTTP transport implementation

#[cfg(all(feature = "client", not(feature = "server")))]
use reqwest::header::HeaderMap;

#[cfg(feature = "http-server")]
use {
    volga::{auth::AuthClaims, headers::HeaderMap},
    server::{AuthConfig, DefaultClaims}
};

use futures_util::TryFutureExt;
use std::{borrow::Cow, fmt::Display};
use tokio_util::sync::CancellationToken;
use tokio::sync::{mpsc::{self, Receiver, Sender}};
use crate::{
    error::{Error, ErrorCode},
    shared::MemChr,
    types::Message
};

use super::{
    Transport,
    Sender as TransportSender,
    Receiver as TransportReceiver
};

#[cfg(all(feature = "http-server", feature = "tls"))]
pub use volga::tls::TlsConfig;

#[cfg(all(feature = "http-client", feature = "tls"))]
use {
    std::path::PathBuf,
    reqwest::Certificate
};

#[cfg(feature = "http-server")]
pub(crate) mod server;
#[cfg(feature = "http-client")]
pub(crate) mod client;

pub(super) const MCP_SESSION_ID: &str = "Mcp-Session-Id";
const DEFAULT_ADDR: &str = "127.0.0.1:3000";
const DEFAULT_MCP_ENDPOINT: &str = "/mcp";

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
    #[cfg(feature = "tls")]
    Https
}

/// Represents HTTP server transport
#[cfg(feature = "http-server")]
pub struct HttpServer<C: AuthClaims = DefaultClaims> {
    url: ServiceUrl,
    auth: Option<AuthConfig<C>>,
    #[cfg(feature = "tls")]
    tls_config: Option<TlsConfig>,
    sender: HttpSender,
    receiver: HttpReceiver,
}

#[cfg(feature = "http-client")]
pub struct HttpClient {
    url: ServiceUrl,
    access_token: Option<Box<[u8]>>,
    #[cfg(feature = "tls")]
    ca_cert: Option<PathBuf>,
    sender: HttpSender,
    receiver: HttpReceiver,
}

#[derive(Clone, Copy)]
pub struct ServiceUrl {
    proto: HttpProto,
    addr: &'static str,
    endpoint: &'static str,
}

#[cfg(feature = "http-server")]
pub(super) struct HttpRuntimeContext {
    url: ServiceUrl,
    tx: Sender<Result<Message, Error>>,
    #[cfg(feature = "tls")]
    tls_config: Option<TlsConfig>,
    rx: Receiver<Message>,
    auth: Option<AuthConfig>,
}

#[cfg(feature = "http-client")]
pub(super) struct ClientRuntimeContext {
    url: ServiceUrl,
    tx: Sender<Result<Message, Error>>,
    rx: Receiver<Message>,
    access_token: Option<Box<[u8]>>,
    #[cfg(feature = "tls")]
    ca_cert: Option<Certificate>,
}

/// Represents HTTP sender
pub(crate) struct HttpSender {
    tx: Sender<Message>,
    rx: Option<Receiver<Message>>,
}

/// Represents HTTP receiver
pub(crate) struct HttpReceiver {
    tx: Sender<Result<Message, Error>>,
    rx: Receiver<Result<Message, Error>>
}

#[cfg(feature = "http-server")]
impl Default for HttpServer {
    #[inline]
    fn default() -> Self {
        Self {
            url: ServiceUrl::default(),
            auth: None,
            #[cfg(feature = "tls")]
            tls_config: None,
            receiver: HttpReceiver::new(),
            sender: HttpSender::new()
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
            #[cfg(feature = "tls")]
            ca_cert: None,
            receiver: HttpReceiver::new(),
            sender: HttpSender::new()
        }
    }
}

impl ServiceUrl {
    #[inline]
    pub fn as_str<'a>(&self) -> Cow<'a, str> {
        Cow::Owned(format!("{}://{}{}", self.proto, self.addr, self.endpoint))
    }
}

impl Display for HttpProto {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self { 
            HttpProto::Http => f.write_str("http"),
            #[cfg(feature = "tls")]
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
    #[cfg(feature = "tls")]
    pub fn with_tls<F>(mut self, config: F) -> Self
    where
        F: FnOnce(TlsConfig) -> TlsConfig
    {
        self.tls_config = Some(config(Default::default()));
        self.url.proto = HttpProto::Https;
        self
    }
    
    /// Configures authentication and authorization
    pub fn with_auth<F>(mut self, config: F) -> Self
    where 
        F: FnOnce(AuthConfig) -> AuthConfig
    {
        self.auth = Some(config(AuthConfig::default()));
        self    
    }
    
    fn runtime(&mut self) -> Result<HttpRuntimeContext, Error> {
        let Some(sender_rx) = self.sender.rx.take() else {
            return Err(Error::new(ErrorCode::InternalError, "The HTTP writer is already in use"));
        };
        Ok(HttpRuntimeContext {
            url: self.url,
            tx: self.receiver.tx.clone(),
            rx: sender_rx,
            auth: self.auth.take(),
            #[cfg(feature = "tls")]
            tls_config: self.tls_config.take(),
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

    #[cfg(feature = "tls")]
    pub fn with_tls(mut self, cert: impl Into<PathBuf>) -> Self {
        self.ca_cert = Some(cert.into());
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
            return Err(Error::new(ErrorCode::InternalError, "The HTTP writer is already in use"));
        };
        Ok(ClientRuntimeContext {
            url: self.url,
            tx: self.receiver.tx.clone(),
            rx: sender_rx,
            access_token: self.access_token.take(),
            #[cfg(feature = "tls")]
            ca_cert: self.ca_cert
                .take()
                .and_then(|p| Certificate::from_pem(&std::fs::read(p)
                    .expect("Expected CA"))
                    .ok())
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
        self.rx
            .recv()
            .await
            .unwrap_or_else(|| Err(Error::new(ErrorCode::InvalidRequest, "Unexpected end of stream")))
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
        tokio::spawn(server::serve(
            runtime,
            token.clone())
        );
        
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
        tokio::spawn(client::connect(
            runtime,
            token.clone()
        ));
        
        token
    }

    fn split(self) -> (Self::Sender, Self::Receiver) {
        (self.sender, self.receiver)
    }
}

#[cfg(test)]
mod test {
    
}
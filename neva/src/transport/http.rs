//! Streamable HTTP transport implementation

#[cfg(all(feature = "client", not(feature = "server")))]
use reqwest::header::HeaderMap;

#[cfg(feature = "server")]
use volga::headers::HeaderMap;

use futures_util::TryFutureExt;
use std::borrow::Cow;
use std::fmt::Display;
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

#[cfg(feature = "server")]
pub(crate) mod server;
#[cfg(feature = "client")]
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

/// Represents HTTP server transport
#[cfg(feature = "server")]
pub struct HttpServer {
    url: ServiceUrl,
    sender: HttpSender,
    receiver: HttpReceiver,
}

#[cfg(feature = "client")]
pub struct HttpClient {
    url: ServiceUrl,
    sender: HttpSender,
    receiver: HttpReceiver,
}

#[derive(Clone, Copy)]
pub struct ServiceUrl {
    addr: &'static str,
    endpoint: &'static str,
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

#[cfg(feature = "server")]
impl Default for HttpServer {
    #[inline]
    fn default() -> Self {
        Self {
            url: ServiceUrl::default(),
            receiver: HttpReceiver::new(),
            sender: HttpSender::new()
        }
    }
}

#[cfg(feature = "client")]
impl Default for HttpClient {
    #[inline]
    fn default() -> Self {
        Self {
            url: ServiceUrl::default(),
            receiver: HttpReceiver::new(),
            sender: HttpSender::new()
        }
    }
}

impl ServiceUrl {
    #[inline]
    pub fn as_str<'a>(&self) -> Cow<'a, str> {
        #[cfg(feature = "tls")]
        let proto = "https";
        #[cfg(not(feature = "tls"))]
        let proto = "http";
        Cow::Owned(format!("{proto}://{}/{}", self.addr, self.endpoint))
    }
}

impl Default for ServiceUrl {
    #[inline]
    fn default() -> Self {
        Self {
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

#[cfg(feature = "server")]
impl HttpServer {
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
    
    /// Returns service URL (IP, port and URL prefix)
    pub(crate) fn url(&self) -> ServiceUrl {
        self.url
    }
}

#[cfg(feature = "client")]
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

    /// Returns service URL (IP, port and URL prefix)
    #[allow(dead_code)]
    pub(crate) fn url(&self) -> ServiceUrl {
        self.url
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

#[cfg(feature = "server")]
impl Transport for HttpServer {
    type Sender = HttpSender;
    type Receiver = HttpReceiver;

    fn start(&mut self) -> CancellationToken {
        let token = CancellationToken::new();
        let Some(sender_rx) = self.sender.rx.take() else {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "The HTTP writer is already in use");
            return token;
        };
        tokio::spawn(server::serve(
            self.url(),
            self.receiver.tx.clone(), 
            sender_rx,
            token.clone())
        );
        
        token
    }

    #[inline]
    fn split(self) -> (Self::Sender, Self::Receiver) {
        (self.sender, self.receiver)
    }
}

#[cfg(feature = "client")]
impl Transport for HttpClient {
    type Sender = HttpSender;
    type Receiver = HttpReceiver;

    fn start(&mut self) -> CancellationToken {
        let token = CancellationToken::new();
        let Some(sender_rx) = self.sender.rx.take() else {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "The HTTP writer is already in use");
            return token;
        };
        tokio::spawn(client::connect(
            self.url(),
            self.receiver.tx.clone(),
            sender_rx,
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
use std::future::Future;
use tokio_util::sync::CancellationToken;
use crate::error::{Error, ErrorCode};
use crate::types::Message;

#[cfg(feature = "server")]
pub(crate) use stdio::StdIoServer;
#[cfg(feature = "http-server")]
pub(crate) use http::HttpServer;

#[cfg(feature = "client")]
pub(crate) use stdio::StdIoClient;

pub(crate) mod stdio;
#[cfg(any(feature = "http-server", feature = "http-client"))]
pub(crate) mod http;

/// Describes a sender that can send messages to a client 
pub(crate) trait Sender {
    /// Sends messages to a client
    fn send(&mut self, resp: Message) -> impl Future<Output = Result<(), Error>>;
}

/// Describes a receiver that can receive messages from a client
pub(crate) trait Receiver {
    /// Receives a messages from a client
    fn recv(&mut self) -> impl Future<Output = Result<Message, Error>>;
}

/// Describes a transport protocol for communicating between server and client
pub(crate) trait Transport {
    type Sender: Sender;
    type Receiver: Receiver;
    
    /// Starts the server with the current transport protocol
    fn start(&mut self) -> CancellationToken;
    
    /// Splits transport into [`Sender`] and [`Receiver`] that can be used in a different threads
    fn split(self) -> (Self::Sender, Self::Receiver);
}

/// Holds all supported transport protocols
pub(crate) enum TransportProto {
    None,
    #[cfg(feature = "client")]
    StdioClient(StdIoClient),
    #[cfg(feature = "server")]
    StdIoServer(StdIoServer),
    #[cfg(feature = "http-server")]
    HttpServer(HttpServer),
    //Ws(Websocket),
    // add more options here...
}

#[derive(Clone)]
pub(crate) enum TransportProtoSender {
    None,
    Stdio(stdio::StdIoSender),
    #[cfg(any(feature = "http-server", feature = "http-client"))]
    Http(http::HttpSender)
}

pub(crate) enum TransportProtoReceiver {
    None,
    Stdio(stdio::StdIoReceiver),
    #[cfg(any(feature = "http-server", feature = "http-client"))]
    Http(http::HttpReceiver)
}

impl Default for TransportProto {
    #[inline]
    fn default() -> Self {
        TransportProto::None
    }
}

impl Sender for TransportProtoSender {
    #[inline]
    async fn send(&mut self, resp: Message) -> Result<(), Error> {
        match self {
            TransportProtoSender::Stdio(stdio) => stdio.send(resp).await,
            #[cfg(any(feature = "http-server", feature = "http-client"))]
            TransportProtoSender::Http(http) => http.send(resp).await,
            TransportProtoSender::None => Err(Error::new(
                ErrorCode::InternalError,
                "Transport protocol must be specified"
            )),
        }
    }
}

impl Receiver for TransportProtoReceiver {
    #[inline]
    async fn recv(&mut self) -> Result<Message, Error> {
        match self {
            TransportProtoReceiver::Stdio(stdio) => stdio.recv().await,
            #[cfg(any(feature = "http-server", feature = "http-client"))]
            TransportProtoReceiver::Http(http) => http.recv().await,
            TransportProtoReceiver::None => Err(Error::new(
                ErrorCode::InternalError,
                "Transport protocol must be specified"
            )),
        }
    }
}

impl Transport for TransportProto {
    type Sender = TransportProtoSender;
    type Receiver = TransportProtoReceiver;
    
    #[inline]
    fn start(&mut self) -> CancellationToken {
        match self {
            #[cfg(feature = "server")]
            TransportProto::StdIoServer(stdio) => stdio.start(),
            #[cfg(feature = "client")]
            TransportProto::StdioClient(stdio) => stdio.start(),
            #[cfg(feature = "http-server")]
            TransportProto::HttpServer(http) => http.start(),
            TransportProto::None => CancellationToken::new(),
        }
    }
    
    fn split(self) -> (Self::Sender, Self::Receiver) {
        match self {
            #[cfg(feature = "server")]
            TransportProto::StdIoServer(stdio) => {
                let (tx, rx) = stdio.split();
                (TransportProtoSender::Stdio(tx), TransportProtoReceiver::Stdio(rx))
            },
            #[cfg(feature = "http-server")]
            TransportProto::HttpServer(http) => {
                let (tx, rx) = http.split();
                (TransportProtoSender::Http(tx), TransportProtoReceiver::Http(rx))
            },
            #[cfg(feature = "client")]
            TransportProto::StdioClient(stdio) => {
                let (tx, rx) = stdio.split();
                (TransportProtoSender::Stdio(tx), TransportProtoReceiver::Stdio(rx))
            },
            TransportProto::None => (TransportProtoSender::None, TransportProtoReceiver::None),
        }
    }
}

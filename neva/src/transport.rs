use std::future::Future;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use crate::error::{Error, ErrorCode};

pub(crate) use stdio::StdIo;

pub(crate) mod stdio;

/// Describes a sender that can send messages to a client 
pub(crate) trait Sender{
    /// Sends messages to a client
    fn send<R: Serialize>(&mut self, resp: R) -> impl Future<Output = Result<(), Error>>;
}

/// Describes a receiver that can receive messages from a client
pub(crate) trait Receiver {
    /// Receives a messages from a client
    fn recv<R: DeserializeOwned>(&mut self) -> impl Future<Output = Result<R, Error>>;
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
    Stdio(StdIo),
    //Ws(Websocket),
    //Sse(Sse),
    // add more options here...
}

#[derive(Clone)]
pub(crate) enum TransportProtoSender {
    None,
    Stdio(stdio::StdIoSender),
}

pub(crate) enum TransportProtoReceiver {
    None,
    Stdio(stdio::StdIoReceiver),
}

impl Default for TransportProto {
    #[inline]
    fn default() -> Self {
        TransportProto::None
    }
}

impl Sender for TransportProtoSender {
    #[inline]
    async fn send<R: Serialize>(&mut self, resp: R) -> Result<(), Error> {
        match self {
            TransportProtoSender::Stdio(stdio) => stdio.send(resp).await,
            TransportProtoSender::None => Err(Error::new(
                ErrorCode::InternalError,
                "Transport protocol must be specified"
            )),
        }
    }
}

impl Receiver for TransportProtoReceiver {
    #[inline]
    async fn recv<R: DeserializeOwned>(&mut self) -> Result<R, Error> {
        match self {
            TransportProtoReceiver::Stdio(stdio) => stdio.recv().await,
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
            TransportProto::Stdio(stdio) => stdio.start(),
            TransportProto::None => CancellationToken::new(),
        }
    }
    
    fn split(self) -> (Self::Sender, Self::Receiver) {
        match self {
            TransportProto::Stdio(stdio) => {
                let (tx, rx) = stdio.split();
                (TransportProtoSender::Stdio(tx), TransportProtoReceiver::Stdio(rx))
            },
            TransportProto::None => (TransportProtoSender::None, TransportProtoReceiver::None),
        }
    }
}

impl Sender for TransportProto {
    #[inline]
    async fn send<R: Serialize>(&mut self, resp: R) -> Result<(), Error> {
        match self {
            TransportProto::Stdio(stdio) => stdio.send(resp).await,
            TransportProto::None => Err(Error::new(
                ErrorCode::InternalError,
                "Transport protocol must be specified"
            )),
        }
    }
}

impl Receiver for TransportProto {
    #[inline]
    async fn recv<R: DeserializeOwned>(&mut self) -> Result<R, Error> {
        match self {
            TransportProto::Stdio(stdio) => stdio.recv().await,
            TransportProto::None => Err(Error::new(
                ErrorCode::InternalError,
                "Transport protocol must be specified"
            )),
        }
    }
}
use std::future::Future;
use crate::error::Error;
use crate::types::{Request, Response};

pub(crate) use stdio::StdIo;

pub(crate) mod stdio;

/// Describes a transport protocol for communicating between server and client
pub(crate) trait Transport {
    /// Starts the server with the current transport protocol
    fn start(&self);

    /// Receives a messages from client
    fn recv(&mut self) -> impl Future<Output = Result<Request, Error>>;

    /// Sends messages to client
    fn send(&mut self, resp: Response) -> impl Future<Output = Result<(), Error>>;
}

/// Holds all supported transport protocols
pub(crate) enum TransportProto {
    None,
    Stdio(StdIo),
    //Ws(Websocket),
    //Sse(Sse),
    // add more options here...
}

impl Default for TransportProto {
    #[inline]
    fn default() -> Self {
        TransportProto::None
    }
}

impl Transport for TransportProto {
    #[inline]
    fn start(&self) {
        match self {
            TransportProto::Stdio(stdio) => stdio.start(),
            TransportProto::None => (),
        };
    }
    
    #[inline]
    async fn recv(&mut self) -> Result<Request, Error> {
        match self {
            TransportProto::Stdio(stdio) => stdio.recv().await,
            TransportProto::None => Err(Error::new("Transport protocol must be specified")),
        }
    }

    #[inline]
    async fn send(&mut self, resp: Response) -> Result<(), Error> {
        match self {
            TransportProto::Stdio(stdio) => stdio.send(resp).await,
            TransportProto::None => Err(Error::new("Transport protocol must be specified")),
        }
    }
}

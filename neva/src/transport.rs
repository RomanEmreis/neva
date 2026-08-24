//! Transport protocols and utilities for communicating between server and client

use crate::error::{Error, ErrorCode};
use crate::types::Message;
use std::future::Future;
use tokio_util::sync::CancellationToken;

#[cfg(feature = "http-server")]
pub use http::{
    EventId, HttpContext, HttpEngine, HttpRequest, HttpResponse, HttpServer, StreamResponse,
    handlers,
};

#[cfg(feature = "http-server")]
#[allow(deprecated)]
pub use http::SseResponse;

#[cfg(feature = "server")]
pub(crate) use stdio::StdIoServer;

#[cfg(feature = "http-client")]
pub(crate) use http::HttpClient;
#[cfg(feature = "client")]
pub(crate) use stdio::StdIoClient;

pub(crate) mod drain;
#[cfg(any(feature = "http-server", feature = "http-client"))]
pub mod http;
pub(crate) mod stdio;

pub(crate) use drain::{DrainGuard, DrainSignal};

/// Describes a sender that can send messages to a client
pub(crate) trait Sender {
    /// Sends messages to a client
    fn send(&mut self, resp: Message) -> impl Future<Output = Result<(), Error>>;
}

/// Describes a receiver that can receive messages from a client
pub(crate) trait Receiver {
    /// Receives messages from a client
    fn recv(&mut self) -> impl Future<Output = Result<Message, Error>>;
}

/// What a transport hands back when it starts: the signal that stops it, and
/// the signal that says it has stopped.
///
/// The two are deliberately separate. Cancelling the token is a request the
/// writers observe; they answer it by draining whatever is already queued and
/// only then dropping their [`DrainGuard`], which is what completes
/// [`drained`](Self::drained). Joining the second to `App::run` returning is
/// what keeps a runtime dropped right after it from aborting the drain -- see
/// [`drain`] for the whole story.
#[derive(Debug)]
pub(crate) struct TransportHandle {
    /// Cancelling this asks the transport to stop.
    pub(crate) token: CancellationToken,
    /// Completes once every writer has drained what was queued and exited.
    ///
    /// Joined by the server, in `App::run`. A client transport carries the
    /// signal all the same -- the stdio writer is one piece of code for both
    /// roles -- but nothing on that side waits on it yet, so the field is
    /// write-only in a client-only build.
    #[cfg_attr(not(feature = "server"), allow(dead_code))]
    pub(crate) drained: DrainSignal,
}

impl TransportHandle {
    /// Pairs a transport's cancellation token with the drain signal its
    /// writers raise.
    #[inline]
    pub(crate) fn new(token: CancellationToken, drained: DrainSignal) -> Self {
        Self { token, drained }
    }

    /// A handle for a transport with no writers of its own to wait for: the
    /// drain signal is complete from the start, so awaiting it costs nothing.
    #[inline]
    pub(crate) fn detached(token: CancellationToken) -> Self {
        Self::new(token, DrainSignal::ready())
    }
}

/// Describes a transport protocol for communicating between server and client
pub(crate) trait Transport {
    type Sender: Sender;
    type Receiver: Receiver;

    /// Starts the server with the current transport protocol
    fn start(&mut self) -> TransportHandle;

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
    HttpServer(Box<dyn http::core::engine::HttpTransport>),
    #[cfg(feature = "http-client")]
    HttpClient(Box<HttpClient>),
    //Ws(Websocket),
    // add more options here...
}

#[derive(Clone)]
pub(crate) enum TransportProtoSender {
    None,
    Stdio(stdio::StdIoSender),
    #[cfg(any(feature = "http-server", feature = "http-client"))]
    Http(http::HttpSender),
    /// Batch-scoped sender that routes `Message::Response` items into an in-memory
    /// collection and forwards everything else (server-initiated requests,
    /// notifications) straight to the real transport.
    #[cfg(feature = "server")]
    BatchCollect {
        /// The underlying transport sender for non-response messages.
        ///
        /// `Arc` makes cloning this sender cheap. `tokio::sync::Mutex`
        /// is required because `Sender::send` takes `&mut self` and the call
        /// crosses an `.await` point.
        real_sender: std::sync::Arc<tokio::sync::Mutex<TransportProtoSender>>,
        /// Accumulated response envelopes to be bundled into the batch reply.
        ///
        /// `std::sync::Mutex` is intentional: the lock is never held across an
        /// `.await` (lock -> push -> unlock, then `Ok(())`), so the lighter
        /// synchronous mutex is the right tool here.
        responses: std::sync::Arc<std::sync::Mutex<Vec<crate::types::MessageEnvelope>>>,
    },
}

pub(crate) enum TransportProtoReceiver {
    None,
    Stdio(stdio::StdIoReceiver),
    #[cfg(any(feature = "http-server", feature = "http-client"))]
    Http(http::HttpReceiver),
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
                "Transport protocol must be specified",
            )),
            #[cfg(feature = "server")]
            TransportProtoSender::BatchCollect {
                real_sender,
                responses,
            } => match resp {
                Message::Response(response) => {
                    if let Ok(mut guard) = responses.lock() {
                        guard.push(crate::types::MessageEnvelope::Response(response));
                    }
                    Ok(())
                }
                other => {
                    let mut guard = real_sender.lock().await;
                    Box::pin(guard.send(other)).await
                }
            },
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
                "Transport protocol must be specified",
            )),
        }
    }
}

impl Transport for TransportProto {
    type Sender = TransportProtoSender;
    type Receiver = TransportProtoReceiver;

    #[inline]
    fn start(&mut self) -> TransportHandle {
        match self {
            #[cfg(feature = "server")]
            TransportProto::StdIoServer(stdio) => stdio.start(),
            #[cfg(feature = "client")]
            TransportProto::StdioClient(stdio) => stdio.start(),
            #[cfg(feature = "http-server")]
            TransportProto::HttpServer(http) => http.start(),
            #[cfg(feature = "http-client")]
            TransportProto::HttpClient(http) => http.start(),
            TransportProto::None => TransportHandle::detached(CancellationToken::new()),
        }
    }

    fn split(self) -> (Self::Sender, Self::Receiver) {
        match self {
            #[cfg(feature = "server")]
            TransportProto::StdIoServer(stdio) => {
                let (tx, rx) = stdio.split();
                (
                    TransportProtoSender::Stdio(tx),
                    TransportProtoReceiver::Stdio(rx),
                )
            }
            #[cfg(feature = "http-server")]
            TransportProto::HttpServer(http) => {
                let (tx, rx) = http.split_into_proto();
                (
                    TransportProtoSender::Http(tx),
                    TransportProtoReceiver::Http(rx),
                )
            }
            #[cfg(feature = "client")]
            TransportProto::StdioClient(stdio) => {
                let (tx, rx) = stdio.split();
                (
                    TransportProtoSender::Stdio(tx),
                    TransportProtoReceiver::Stdio(rx),
                )
            }
            #[cfg(feature = "http-client")]
            TransportProto::HttpClient(http) => {
                let (tx, rx) = http.split();
                (
                    TransportProtoSender::Http(tx),
                    TransportProtoReceiver::Http(rx),
                )
            }
            TransportProto::None => (TransportProtoSender::None, TransportProtoReceiver::None),
        }
    }
}

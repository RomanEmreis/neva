//! The [`HttpEngine`] contract — what an HTTP-stack adapter must implement.

use crate::{error::Error, types::Message};
use std::future::Future;
use tokio_util::sync::CancellationToken;

use super::{
    context::HttpContext,
    types::{HttpRequest, HttpResponse},
};

/// Contract for an HTTP framework adapter.
///
/// The engine declares its native request/response types, supplies two
/// HTTP conversion bridges and two SSE event constructors, and runs an
/// HTTP server until `token` fires. All JSON-RPC framing, SSE
/// replay/dedup, batch fast-path, and oneshot pending logic stays in
/// neva — an engine adapter is the thinnest possible shim from neva's
/// neutral types onto a framework's native types.
///
/// Route handlers typically just call the `dispatch_*` helpers in
/// [`super::handlers`], which compose conversion + protocol dispatch +
/// conversion-back in one call.
///
/// # Example
///
/// ```rust,ignore
/// struct MyEngine;
///
/// impl HttpEngine for MyEngine {
///     type Request  = framework::Request;
///     type Response = framework::Response;
///     type SseEvent = framework::sse::Event;
///
///     async fn into_neutral(req: Self::Request) -> HttpRequest { ... }
///     fn into_engine(resp: HttpResponse) -> Self::Response { ... }
///
///     fn sse_tracked(seq: u64, msg: &Message) -> Self::SseEvent { ... }
///     fn sse_ephemeral(msg: &Message) -> Self::SseEvent { ... }
///
///     async fn run(self, ctx: HttpContext, token: CancellationToken)
///         -> Result<(), Error> { ... }
/// }
/// ```
pub trait HttpEngine: Send + Sync + 'static {
    /// Engine-native inbound request type (e.g. `axum::Request<Body>`).
    type Request: Send + 'static;

    /// Engine-native outbound response type (e.g. `axum::Response`).
    type Response: Send + 'static;

    /// Engine-native SSE event type (e.g. `volga::http::sse::Message`).
    type SseEvent: Send + 'static;

    /// Convert an engine-native request into neva's neutral
    /// [`HttpRequest`]. The body must be fully buffered before return.
    fn into_neutral(req: Self::Request) -> impl Future<Output = HttpRequest> + Send;

    /// Build an engine-native response from neva's neutral
    /// [`HttpResponse`].
    fn into_engine(resp: HttpResponse) -> Self::Response;

    /// Build an SSE event WITH an `id:` field (advances the client's
    /// `Last-Event-ID`, eligible for replay on reconnect).
    fn sse_tracked(seq: u64, msg: &Message) -> Self::SseEvent;

    /// Build an SSE event WITHOUT an `id:` field (ephemeral
    /// log / notification).
    fn sse_ephemeral(msg: &Message) -> Self::SseEvent;

    /// Run the HTTP server until `token` fires.
    fn run(
        self,
        ctx: HttpContext,
        token: CancellationToken,
    ) -> impl Future<Output = Result<(), Error>> + Send;
}

/// `pub(crate)` dyn-compatible bridge: lets `TransportProto::HttpServer`
/// store any `HttpServer<C, E>` behind a single trait object without
/// becoming generic itself. Engines never see this trait — it lives at
/// the `HttpServer` ⇄ `TransportProto` seam.
pub(crate) trait HttpTransport: Send + Sync + 'static {
    /// Starts the engine and returns a token that, when cancelled, shuts
    /// it down.
    fn start(&mut self) -> CancellationToken;
    /// Consumes the transport into its split (sender, receiver) halves
    /// for use by the App's main loop.
    fn split_into_proto(
        self: Box<Self>,
    ) -> (
        crate::transport::http::HttpSender,
        crate::transport::http::HttpReceiver,
    );
    /// Human-readable URL label for the greeting banner.
    fn url_label(&self) -> String;
}

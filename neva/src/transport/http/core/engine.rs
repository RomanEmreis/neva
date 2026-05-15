//! The [`HttpEngine`] contract — what an HTTP-stack adapter must implement.

use crate::error::Error;
use std::future::Future;
use tokio_util::sync::CancellationToken;

use super::{
    context::HttpContext,
    types::{HttpRequest, HttpResponse, SseResponder},
};

/// Contract for an HTTP framework adapter.
///
/// The engine declares its native request/response types, supplies the
/// two conversion bridges, and runs an HTTP server until `token` fires.
/// All JSON-RPC framing, SSE replay/dedup, batch fast-path, and oneshot
/// pending logic stays in neva — an engine adapter is the thinnest
/// possible shim from neva's neutral types onto a framework's native
/// types.
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
///     type Request     = framework::Request;
///     type Response    = framework::Response;
///     type SseResponder = MyResponder;
///
///     async fn into_neutral(req: Self::Request) -> HttpRequest { ... }
///     fn into_engine(resp: HttpResponse) -> Self::Response { ... }
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

    /// Bridge that builds engine-native SSE events from MCP messages.
    type SseResponder: SseResponder + Clone;

    /// Convert an engine-native request into neva's neutral
    /// [`HttpRequest`]. The body must be fully buffered before return.
    fn into_neutral(req: Self::Request) -> impl Future<Output = HttpRequest> + Send;

    /// Build an engine-native response from neva's neutral
    /// [`HttpResponse`].
    fn into_engine(resp: HttpResponse) -> Self::Response;

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

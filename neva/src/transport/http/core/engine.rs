//! The [`HttpEngine`] contract — what an HTTP-stack adapter must implement.

use crate::error::Error;
use std::{future::Future, sync::Arc};
use tokio_util::sync::CancellationToken;

use super::{context::HttpContext, types::SseResponder};

/// Contract for an HTTP framework adapter.
///
/// One method. The engine binds to `ctx.addr`, registers three routes
/// (`POST`, `GET`, `DELETE` on `ctx.endpoint`), calls neva's helpers in
/// [`super::handlers`] inside its route handlers, and runs until `token`
/// fires.
///
/// All protocol logic — JSON-RPC framing, SSE replay/dedup, batch
/// fast-path, oneshot pending — lives in neva. An engine adapter is the
/// thinnest possible shim from neva's neutral types onto the framework's
/// native types.
///
/// # Example
///
/// ```rust,ignore
/// struct MyEngine;
///
/// impl HttpEngine for MyEngine {
///     type SseResponder = MyResponder;
///     async fn run(
///         self,
///         ctx: Arc<HttpContext>,
///         token: CancellationToken,
///     ) -> Result<(), Error> {
///         // bind ctx.addr, wire routes on ctx.endpoint, call handlers::*
///         Ok(())
///     }
/// }
/// ```
pub trait HttpEngine: Send + Sync + 'static {
    /// Engine-native SSE event type producer.
    type SseResponder: SseResponder;

    /// Run the HTTP server until `token` fires.
    fn run(
        self,
        ctx: Arc<HttpContext>,
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

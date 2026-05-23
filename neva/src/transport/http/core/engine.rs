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
/// # Authorization contract
///
/// To enable neva's per-tool / per-prompt / per-resource role and
/// permission gates, the engine is responsible for decoding the
/// inbound request's auth credential (bearer token, session cookie,
/// custom header — whatever the engine supports) into a value
/// implementing [`crate::auth::Claims`], wrapping it in
/// `Arc<dyn neva::auth::Claims>`, and inserting it into
/// `request.extensions_mut()` **before** calling [`dispatch_post`].
///
/// neva itself does not parse credentials — that is the engine's job.
/// If no claims are inserted, neva treats the request as unauthenticated
/// and any tool/prompt/resource that declares required roles or
/// permissions will reject it.
///
/// The default `VolgaEngine` (selected by `HttpServer::default()` /
/// `with_default_http`) does this automatically using Volga's
/// `BearerTokenService`. A custom engine adapter wires up the equivalent
/// step in its own POST route.
///
/// [`dispatch_post`]: super::handlers::dispatch_post
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
///     async fn adapt_request(req: Self::Request) -> Result<HttpRequest, Error> { ... }
///     fn adapt_response(resp: HttpResponse) -> Self::Response { ... }
///
///     fn tracked_event(seq: u64, msg: &Message) -> Self::SseEvent { ... }
///     fn ephemeral_event(msg: &Message) -> Self::SseEvent { ... }
///
///     async fn run(self, ctx: HttpContext, token: CancellationToken)
///         -> Result<(), Error> { ... }
/// }
/// ```
pub trait HttpEngine: Send + Sync + 'static {
    /// Engine-native inbound request type (e.g. `axum::Request<Body>`).
    ///
    /// `Send` is intentionally not required here, so engines whose native
    /// request type holds a non-`Send` state (actix-web's `HttpRequest`
    /// holds `Rc<…>` internally) can still implement this trait. For
    /// engines whose request type is `Send`, [`dispatch_post`] /
    /// [`dispatch_delete`] / [`dispatch_get_sse`] produce `Send` futures
    /// automatically; for `!Send` engines, the futures are `!Send` and
    /// the engine is expected to await them on its own runtime
    /// (typically a per-thread `LocalSet`) without `tokio::spawn`.
    ///
    /// [`dispatch_post`]: super::handlers::dispatch_post
    /// [`dispatch_delete`]: super::handlers::dispatch_delete
    /// [`dispatch_get_sse`]: super::handlers::dispatch_get_sse
    type Request;

    /// Engine-native outbound response type (e.g. `axum::Response`).
    ///
    /// Same `Send` story as [`Self::Request`].
    type Response;

    /// Engine-native SSE event type (e.g. `volga::http::sse::Message`).
    ///
    /// `Send` is required because SSE events are yielded by an
    /// engine-agnostic `impl Stream<Item = Self::SseEvent> + Send`
    /// returned from [`super::handlers::handle_get_sse`].
    type SseEvent: Send;

    /// Convert an engine-native request into neva's neutral
    /// [`HttpRequest`]. The body must be fully buffered before return.
    ///
    /// Returns [`Err`] when the body cannot be read or the neutral request
    /// cannot be constructed (e.g. an invalid URI / method emitted by the
    /// underlying stack). The error is propagated through
    /// [`super::handlers::dispatch_post`] / [`dispatch_delete`] /
    /// [`dispatch_get_sse`] so the engine can map it onto its native
    /// failure mode without ever needing to `unwrap` / `expect`.
    ///
    /// [`dispatch_delete`]: super::handlers::dispatch_delete
    /// [`dispatch_get_sse`]: super::handlers::dispatch_get_sse
    fn adapt_request(req: Self::Request) -> impl Future<Output = Result<HttpRequest, Error>>;

    /// Build an engine-native response from neva's neutral
    /// [`HttpResponse`].
    fn adapt_response(resp: HttpResponse) -> Self::Response;

    /// Build an SSE event WITH an `id:` field (advances the client's
    /// `Last-Event-ID`, eligible for replay on reconnect).
    fn tracked_event(seq: u64, msg: &Message) -> Self::SseEvent;

    /// Build an SSE event WITHOUT an `id:` field (ephemeral
    /// log / notification).
    fn ephemeral_event(msg: &Message) -> Self::SseEvent;

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

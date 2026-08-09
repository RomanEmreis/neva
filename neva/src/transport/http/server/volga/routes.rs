//! Volga route shells -- each is the thinnest possible bridge from a
//! `volga::HttpRequest` into the engine-agnostic helpers in
//! [`crate::transport::http::core::handlers`].
//!
//! Conversion (`HttpRequest` ↔ neutral, neutral response ↔
//! `HttpResult`) lives on [`super::engine::VolgaEngine`] via
//! [`HttpEngine::adapt_request`] / [`HttpEngine::adapt_response`]; these
//! routes call those methods so the seam matches every other engine.

use crate::transport::http::core::{
    context::HttpContext, engine::HttpEngine, handlers, types::StreamResponse,
};
use ::volga::{
    HttpRequest, HttpResult, di::Dc, error::Error as VolgaError, http::sse::Message as SseMessage,
    sse,
};

use super::engine::VolgaEngine;

// Extractor ordering matters in these signatures: `Dc<T>` reads the request
// parts by reference (`Source::Parts`), while `HttpRequest` consumes them
// (`Source::Request`, a `parts.take()`). Volga resolves handler arguments
// left to right, so `Dc<HttpContext>` must come before `HttpRequest` --
// swapped around, the `Dc` extractor finds nothing to read from.

/// `POST /<endpoint>` -- JSON-RPC ingress.
///
/// The same two-arm shape as [`get`]: `dispatch_post` yields a
/// [`StreamResponse`], where `Stream` is a request-scoped SSE reply (2026-07-28:
/// the request's `notifications/message` / `notifications/progress`
/// followed by its response) and `Complete` is a single-body reply.
/// Claims are attached inside [`VolgaEngine::adapt_request`], per the
/// `HttpEngine` contract.
pub(crate) async fn post(manager: Dc<HttpContext>, req: HttpRequest) -> HttpResult {
    let outcome = handlers::dispatch_post::<VolgaEngine>(req, &manager)
        .await
        .map_err(to_volga_err)?;
    match outcome {
        StreamResponse::Stream { stream, .. } => {
            let stream = futures_util::StreamExt::map(stream, Ok::<SseMessage, VolgaError>);
            sse!(stream; [crate::transport::http::CONTENT_TYPE_OPTIONS])
        }
        StreamResponse::Complete(resp) => VolgaEngine::adapt_response(resp),
    }
}

/// `DELETE /<endpoint>` -- explicit session termination.
///
/// Not routed under MCP 2026-07-28 (stateless: no sessions); kept
/// compiled for the legacy build.
#[cfg_attr(not(feature = "legacy-spec"), allow(dead_code))]
pub(crate) async fn delete(manager: Dc<HttpContext>, req: HttpRequest) -> HttpResult {
    let neutral = VolgaEngine::adapt_request(req)
        .await
        .map_err(to_volga_err)?;
    let resp = handlers::handle_delete(neutral, &manager).await;
    VolgaEngine::adapt_response(resp)
}

/// `GET /<endpoint>` -- SSE subscribe.
///
/// Not routed under MCP 2026-07-28 (stateless: no SSE GET stream);
/// kept compiled for the legacy build.
#[cfg_attr(not(feature = "legacy-spec"), allow(dead_code))]
pub(crate) async fn get(manager: Dc<HttpContext>, req: HttpRequest) -> HttpResult {
    let outcome = handlers::dispatch_get_sse::<VolgaEngine>(req, &manager)
        .await
        .map_err(to_volga_err)?;
    match outcome {
        StreamResponse::Stream { headers, stream } => {
            let session_id = headers
                .get(handlers::MCP_SESSION_ID)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let stream = futures_util::StreamExt::map(stream, Ok::<SseMessage, VolgaError>);
            if let Some(id) = session_id {
                sse!(
                    stream;
                    [
                        (handlers::MCP_SESSION_ID, id),
                        crate::transport::http::CONTENT_TYPE_OPTIONS,
                    ]
                )
            } else {
                sse!(stream; [crate::transport::http::CONTENT_TYPE_OPTIONS])
            }
        }
        StreamResponse::Complete(resp) => VolgaEngine::adapt_response(resp),
    }
}

/// `GET /.well-known/oauth-protected-resource[/<endpoint>]` -- the RFC 9728
/// Protected Resource Metadata document.
///
/// Publicly reachable by design: authorization is scoped to the MCP
/// endpoint group, and RFC 9728 section 3 requires the metadata to be fetchable
/// without credentials -- it is what a client reads to find out *how* to
/// authenticate.
#[cfg(feature = "server-oauth")]
pub(crate) async fn oauth_metadata(manager: Dc<HttpContext>) -> HttpResult {
    VolgaEngine::adapt_response(handlers::handle_oauth_metadata(&manager))
}

/// Map a neva `Error` raised by engine-agnostic helpers onto a Volga
/// server-error so the route can short-circuit with `?` into `HttpResult`.
fn to_volga_err(err: crate::error::Error) -> VolgaError {
    VolgaError::server_error(err.to_string())
}

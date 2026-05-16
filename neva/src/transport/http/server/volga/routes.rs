//! Volga route shells — each is the thinnest possible bridge from a
//! `volga::HttpRequest` into the engine-agnostic helpers in
//! [`crate::transport::http::core::handlers`].
//!
//! Conversion (`HttpRequest` ↔ neutral, neutral response ↔
//! `HttpResult`) lives on [`super::engine::VolgaEngine`] via
//! [`HttpEngine::into_neutral`] / [`HttpEngine::into_engine`]; these
//! routes call those methods so the seam matches every other engine.

use crate::transport::http::core::{
    context::HttpContext, engine::HttpEngine, handlers, types::SseResponse,
};
use ::volga::{
    HttpRequest, HttpResult,
    auth::{Bearer, BearerTokenService},
    di::Dc,
    error::Error as VolgaError,
    headers::AUTHORIZATION,
    http::sse::Message as SseMessage,
    sse,
};
use std::sync::Arc;

use super::engine::VolgaEngine;
use crate::auth::Claims;
use crate::transport::http::core::types::DefaultClaims;

/// Extract the `Authorization` header and decode it into [`DefaultClaims`]
/// using the configured [`BearerTokenService`], if any.
fn decode_claims(
    bts: Option<BearerTokenService>,
    headers: &http::HeaderMap,
) -> Option<DefaultClaims> {
    let bts = bts?;
    let header = headers.get(AUTHORIZATION)?;
    let bearer = Bearer::try_from(header).ok()?;
    bts.decode::<DefaultClaims>(bearer).ok()
}

/// `POST /<endpoint>` — JSON-RPC ingress.
pub(crate) async fn post(req: HttpRequest) -> HttpResult {
    let manager: Dc<Arc<HttpContext>> = req.extract()?;
    let bts: Option<BearerTokenService> = req.extract()?;

    let mut neutral = VolgaEngine::into_neutral(req).await;

    // Stash claims (if any) in the neutral request's extensions so the
    // engine-agnostic handler can attach them to the outgoing message.
    // The wire shape here is `Arc<dyn Claims>`, matching the contract
    // documented on `HttpEngine` — every engine inserts its own
    // `Claims`-implementing type wrapped in `Arc<dyn Claims>` so neva's
    // per-tool/prompt/resource gates run identically across engines.
    if let Some(claims) = decode_claims(bts, neutral.headers()) {
        let claims: Arc<dyn Claims> = Arc::new(claims);
        neutral.extensions_mut().insert(claims);
    }

    let resp = handlers::handle_post(neutral, &manager).await;
    VolgaEngine::into_engine(resp)
}

/// `DELETE /<endpoint>` — explicit session termination.
pub(crate) async fn delete(req: HttpRequest) -> HttpResult {
    let manager: Dc<Arc<HttpContext>> = req.extract()?;
    let neutral = VolgaEngine::into_neutral(req).await;
    let resp = handlers::handle_delete(neutral, &manager).await;
    VolgaEngine::into_engine(resp)
}

/// `GET /<endpoint>` — SSE subscribe.
pub(crate) async fn get(req: HttpRequest) -> HttpResult {
    let manager: Dc<Arc<HttpContext>> = req.extract()?;
    let outcome = handlers::dispatch_get_sse::<VolgaEngine>(req, &manager).await;
    match outcome {
        SseResponse::Stream { headers, stream } => {
            let session_id = headers
                .get(handlers::MCP_SESSION_ID)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let stream = futures_util::StreamExt::map(stream, Ok::<SseMessage, VolgaError>);
            if let Some(id) = session_id {
                sse!(stream; [(handlers::MCP_SESSION_ID, id)])
            } else {
                sse!(stream)
            }
        }
        SseResponse::Status(resp) => VolgaEngine::into_engine(resp),
    }
}

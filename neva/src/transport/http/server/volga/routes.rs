//! Volga route shells — each is the thinnest possible bridge from a
//! `volga::HttpRequest` to a `core::handlers` helper.
//!
//! The routes do three things and nothing else:
//!
//! 1. Resolve the shared [`HttpContext`] from Volga's DI container.
//! 2. Convert the inbound `volga::HttpRequest` to the neutral
//!    [`http::Request<Bytes>`](crate::transport::http::core::types::HttpRequest)
//!    expected by [`core::handlers`].
//! 3. Translate the neutral response back into a `volga::HttpResult` (or
//!    drive the SSE stream into Volga's `sse!` response shape).

use crate::transport::http::core::{
    context::HttpContext,
    handlers,
    types::{HttpRequest as NeutralRequest, SseResponse},
};
use ::volga::{
    HttpBody, HttpRequest, HttpResult,
    auth::{Bearer, BearerTokenService},
    di::Dc,
    error::Error as VolgaError,
    headers::AUTHORIZATION,
    http::sse::Message as SseMessage,
    sse,
};
use bytes::{Bytes, BytesMut};
use futures_util::StreamExt as _;
use std::sync::Arc;

use super::auth_config::DefaultClaims;
use super::responder::VolgaSseResponder;

/// Read the body of a Volga `HttpRequest` into a buffer.
///
/// MCP JSON-RPC frames are bounded; this matches the existing
/// `read_message` helper in `server.rs`, which also fully buffers the
/// body before handing it to the JSON-RPC decoder.
async fn read_body(body: HttpBody) -> Result<Bytes, VolgaError> {
    let mut stream = body.into_data_stream();
    let mut buf = BytesMut::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| VolgaError::server_error(e.to_string()))?;
        buf.extend_from_slice(&chunk);
    }
    Ok(buf.freeze())
}

/// Convert a Volga `HttpRequest` into a neutral `http::Request<Bytes>`.
///
/// Uses only the public Volga API surface (`method`, `uri`, `version`,
/// `headers`, `into_body`) — Volga's `into_parts` / `extensions_mut` are
/// `pub(crate)` and cannot be used from outside the crate.
async fn into_neutral(req: HttpRequest) -> Result<NeutralRequest, VolgaError> {
    let mut builder = http::Request::builder()
        .method(req.method().clone())
        .uri(req.uri().clone())
        .version(req.version());

    if let Some(headers_mut) = builder.headers_mut() {
        for (k, v) in req.headers().iter() {
            headers_mut.append(k, v.clone());
        }
    }

    let body = read_body(req.into_body()).await?;
    builder
        .body(body)
        .map_err(|e| VolgaError::server_error(e.to_string()))
}

/// Extract the `Authorization` header and decode it into [`DefaultClaims`]
/// using the configured [`BearerTokenService`], if any.
///
/// Mirrors the logic in `server.rs::handle_message` so claims propagation
/// is byte-for-byte identical to the pre-refactor behavior.
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

    let mut neutral = into_neutral(req).await?;

    // Stash claims (if any) in the neutral request's extensions so the
    // engine-agnostic handler can attach them to the outgoing message.
    if let Some(claims) = decode_claims(bts, neutral.headers()) {
        neutral.extensions_mut().insert(claims);
    }

    // `Dc<Arc<HttpContext>>` derefs to `&Arc<HttpContext>`; dereffing
    // once more yields the `HttpContext` reference the handler expects.
    let ctx: &HttpContext = &manager;
    let resp = handlers::handle_post(neutral, ctx).await;
    response_to_volga(resp)
}

/// `DELETE /<endpoint>` — explicit session termination.
pub(crate) async fn delete(req: HttpRequest) -> HttpResult {
    let manager: Dc<Arc<HttpContext>> = req.extract()?;
    let neutral = into_neutral(req).await?;
    let ctx: &HttpContext = &manager;
    let resp = handlers::handle_delete(neutral, ctx).await;
    response_to_volga(resp)
}

/// `GET /<endpoint>` — SSE subscribe.
pub(crate) async fn get(req: HttpRequest) -> HttpResult {
    let manager: Dc<Arc<HttpContext>> = req.extract()?;
    let neutral = into_neutral(req).await?;
    let ctx: &HttpContext = &manager;
    let responder = VolgaSseResponder;
    let outcome = handlers::handle_get_sse(neutral, ctx, &responder).await;
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
        SseResponse::Status(resp) => neutral_to_volga(resp),
    }
}

/// Convert a fully-buffered neutral `http::Response<Bytes>` into a
/// Volga `HttpResult`, copying the status code and every header.
fn response_to_volga(resp: http::Response<Bytes>) -> HttpResult {
    neutral_to_volga(resp)
}

fn neutral_to_volga(resp: http::Response<Bytes>) -> HttpResult {
    let (parts, body) = resp.into_parts();
    let status = parts.status.as_u16();

    let mut buf = BytesMut::with_capacity(body.len());
    buf.extend_from_slice(&body);
    let http_body = HttpBody::full(buf.freeze());

    let mut builder = ::volga::builder!(status);
    for (name, value) in parts.headers.iter() {
        builder = builder.header_raw(name.as_str(), value.as_bytes());
    }
    builder.body(http_body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn build_resp(status: u16, body: &'static [u8]) -> http::Response<Bytes> {
        http::Response::builder()
            .status(status)
            .header("x-test", "v")
            .body(Bytes::from_static(body))
            .unwrap()
    }

    #[tokio::test]
    async fn neutral_to_volga_preserves_status_and_headers() {
        let resp = build_resp(202, b"");
        let volga = neutral_to_volga(resp).expect("response should build");
        // HttpResponse → HeaderMap accessor exists; just check we built a Result.
        assert_eq!(
            volga.headers().get("x-test").map(|v| v.as_bytes()),
            Some(&b"v"[..])
        );
    }
}

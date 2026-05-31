//! [`VolgaEngine`] — the default [`HttpEngine`] implementation.
//!
//! This engine is bound by `HttpServer` when the `http-server-volga`
//! feature is enabled. It owns the Volga adapter logic exclusively: any
//! engine-agnostic JSON-RPC / SSE behavior lives in
//! [`crate::transport::http::core`].

use super::auth_config::AuthConfig;
use crate::error::{Error, ErrorCode};
use crate::transport::http::core::{
    context::HttpContext,
    engine::HttpEngine,
    types::{HttpRequest as NeutralRequest, HttpResponse as NeutralResponse},
};
use crate::types::Message;
#[cfg(feature = "server-tls")]
use ::volga::tls::TlsConfig;
use ::volga::{App, HttpBody, HttpRequest, HttpResult, http::sse::Message as SseMessage};
use bytes::BytesMut;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use super::routes;

/// Default HTTP engine backed by [Volga](https://docs.rs/volga).
///
/// The engine binds a `volga::App` to `ctx.addr`, registers the three
/// MCP routes under `ctx.endpoint`, and delegates every byte of protocol
/// logic to the engine-agnostic helpers in
/// [`crate::transport::http::core::handlers`].
///
/// # Example
///
/// ```rust,ignore
/// use neva::transport::http::server::volga::VolgaEngine;
///
/// let engine = VolgaEngine::default();
/// // wired into `HttpServer` by Task 13 — engines never run standalone.
/// ```
#[derive(Default)]
pub struct VolgaEngine {
    pub(crate) auth: Option<AuthConfig>,
    #[cfg(feature = "server-tls")]
    pub(crate) tls: Option<TlsConfig>,
}

impl std::fmt::Debug for VolgaEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VolgaEngine")
            .field("auth", &self.auth.is_some())
            .finish()
    }
}

impl HttpEngine for VolgaEngine {
    type Request = HttpRequest;
    type Response = HttpResult;
    type SseEvent = SseMessage;

    async fn adapt_request(req: Self::Request) -> Result<NeutralRequest, Error> {
        let mut builder = http::Request::builder()
            .method(req.method().clone())
            .uri(req.uri().clone())
            .version(req.version());

        if let Some(headers_mut) = builder.headers_mut() {
            for (k, v) in req.headers().iter() {
                headers_mut.append(k, v.clone());
            }
        }

        let body = read_body(req.into_body())
            .await
            .map_err(|e| Error::new(ErrorCode::InternalError, e.to_string()))?;
        builder
            .body(body)
            .map_err(|e| Error::new(ErrorCode::InternalError, e.to_string()))
    }

    fn adapt_response(resp: NeutralResponse) -> Self::Response {
        let (parts, body) = resp.into_parts();
        let status = parts.status.as_u16();
        let http_body = HttpBody::full(body);

        let mut builder = ::volga::builder!(status);
        for (name, value) in parts.headers.iter() {
            builder = builder.header_raw(name.as_str(), value.as_bytes());
        }
        builder.body(http_body)
    }

    fn tracked_event(seq: u64, msg: &Message) -> Self::SseEvent {
        SseMessage::new().id(seq.to_string()).json(msg)
    }

    fn ephemeral_event(msg: &Message) -> Self::SseEvent {
        SseMessage::new().json(msg)
    }

    async fn run(self, ctx: HttpContext, token: CancellationToken) -> Result<(), Error> {
        // Volga wires shared state through DI as `Arc<HttpContext>`, so
        // wrap once here for the duration of the engine's lifetime.
        let ctx = Arc::new(ctx);
        let addr = ctx.addr().to_owned();
        let endpoint = ctx.endpoint().to_owned();

        let mut server = App::new()
            .bind(addr.as_str())
            .with_no_delay()
            .without_greeter();

        if let Some(auth) = self.auth {
            let (bearer, rules) = auth.into_parts();
            server = server.with_bearer_auth(|_| bearer);
            server.authorize(rules);
        }

        #[cfg(feature = "server-tls")]
        if let Some(tls) = self.tls {
            server = server.set_tls(tls);
        }

        server
            .add_singleton(ctx)
            .map_err(handle_http_error)
            .group(endpoint.as_str(), |mcp| {
                mcp.map_post("/", routes::post);
                // Stateless RC transport has no SSE GET stream and no
                // session-termination DELETE — only POST is routed.
                #[cfg(not(feature = "proto-2026-07-28-rc"))]
                {
                    mcp.map_get("/", routes::get);
                    mcp.map_delete("/", routes::delete);
                }
            });

        if let Err(e) = server.run().await {
            token.cancel();
            return Err(Error::new(ErrorCode::InternalError, e.to_string()));
        }
        Ok(())
    }
}

/// Read the body of a Volga `HttpRequest` into a buffer.
///
/// MCP JSON-RPC frames are bounded; falling back to an empty body on
/// transport failure lets the protocol layer reply with a clean
/// JSON-RPC `ParseError` instead of a 500.
async fn read_body(body: HttpBody) -> Result<bytes::Bytes, ::volga::error::Error> {
    use futures_util::StreamExt as _;
    let mut stream = body.into_data_stream();
    let mut buf = BytesMut::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ::volga::error::Error::server_error(e.to_string()))?;
        buf.extend_from_slice(&chunk);
    }
    Ok(buf.freeze())
}

async fn handle_http_error(_err: ::volga::error::Error) {
    #[cfg(feature = "tracing")]
    tracing::error!(logger = "neva", "HTTP error: {:?}", _err);
}

//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector
//!
//! cargo run -p example-axum
//! ```
//!
//! This example shows how to plug a non-default HTTP stack — here, axum —
//! into neva's Streamable HTTP transport. It pulls in `neva` with only the
//! engine-agnostic `http-server` feature (no Volga in deps), implements
//! the [`HttpEngine`] contract for an `AxumEngine`, and wires it into
//! `HttpServer::from_engine`.

use std::{convert::Infallible, future::Future, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::State,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::post,
};
use http_body_util::BodyExt;
use neva::{
    error::{Error, ErrorCode},
    prelude::*,
    transport::HttpServer,
    transport::http::core::{
        context::HttpContext,
        engine::HttpEngine,
        handlers,
        types::{HttpRequest, HttpResponse, SseResponder, SseResponse},
    },
    types::Message,
};
use tokio_util::sync::CancellationToken;

/// Engine-side SSE responder — emits axum-native `Event` values directly.
#[derive(Clone, Copy, Debug, Default)]
struct AxumSseResponder;

impl SseResponder for AxumSseResponder {
    type Event = Result<Event, Infallible>;

    fn tracked(&self, seq: u64, msg: &Message) -> Self::Event {
        Ok(Event::default()
            .id(seq.to_string())
            .json_data(msg)
            .unwrap_or_default())
    }

    fn ephemeral(&self, msg: &Message) -> Self::Event {
        Ok(Event::default().json_data(msg).unwrap_or_default())
    }
}

/// HTTP engine backed by [axum](https://docs.rs/axum).
#[derive(Default, Debug)]
struct AxumEngine;

impl HttpEngine for AxumEngine {
    type SseResponder = AxumSseResponder;

    fn run(
        self,
        ctx: Arc<HttpContext>,
        token: CancellationToken,
    ) -> impl Future<Output = Result<(), Error>> + Send {
        async move {
            let addr = ctx.addr();
            let endpoint = ctx.endpoint();

            let app = Router::new()
                .route(
                    endpoint,
                    post(post_handler).get(get_handler).delete(delete_handler),
                )
                .with_state(ctx);

            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .map_err(|e| Error::new(ErrorCode::InternalError, e.to_string()))?;

            axum::serve(listener, app)
                .with_graceful_shutdown(async move { token.cancelled().await })
                .await
                .map_err(|e| Error::new(ErrorCode::InternalError, e.to_string()))
        }
    }
}

async fn post_handler(
    State(ctx): State<Arc<HttpContext>>,
    req: axum::http::Request<Body>,
) -> Response {
    let neutral = into_neutral(req).await;
    let resp = handlers::handle_post(neutral, &ctx).await;
    from_neutral(resp)
}

async fn delete_handler(
    State(ctx): State<Arc<HttpContext>>,
    req: axum::http::Request<Body>,
) -> Response {
    let neutral = into_neutral(req).await;
    let resp = handlers::handle_delete(neutral, &ctx).await;
    from_neutral(resp)
}

async fn get_handler(
    State(ctx): State<Arc<HttpContext>>,
    req: axum::http::Request<Body>,
) -> Response {
    let neutral = into_neutral(req).await;
    let outcome = handlers::handle_get_sse(neutral, &ctx, &AxumSseResponder).await;
    match outcome {
        SseResponse::Stream { headers, stream } => {
            let sse = Sse::new(stream).keep_alive(KeepAlive::default());
            let mut response: Response = sse.into_response();
            for (name, value) in headers.iter() {
                response.headers_mut().insert(name, value.clone());
            }
            response
        }
        SseResponse::Status(resp) => from_neutral(resp),
    }
}

/// Convert axum's `Request<Body>` into the neutral `http::Request<Bytes>`
/// that neva's protocol helpers consume.
async fn into_neutral(req: axum::http::Request<Body>) -> HttpRequest {
    let (parts, body) = req.into_parts();
    let bytes = body
        .collect()
        .await
        .map(|c| c.to_bytes())
        .unwrap_or_default();

    let mut builder = http::Request::builder()
        .method(parts.method)
        .uri(parts.uri)
        .version(parts.version);
    if let Some(headers) = builder.headers_mut() {
        for (name, value) in parts.headers.iter() {
            headers.append(name, value.clone());
        }
    }
    builder.body(bytes).expect("valid request")
}

/// Convert neva's neutral `http::Response<Bytes>` into an axum response.
fn from_neutral(resp: HttpResponse) -> Response {
    let (parts, body) = resp.into_parts();
    let mut builder = http::Response::builder()
        .status(parts.status)
        .version(parts.version);
    if let Some(headers) = builder.headers_mut() {
        for (name, value) in parts.headers.iter() {
            headers.append(name, value.clone());
        }
    }
    builder.body(Body::from(body)).expect("valid response")
}

#[tool]
async fn hello(name: String) -> String {
    format!("Hello, {name}!")
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let http = HttpServer::from_engine("127.0.0.1:3000", AxumEngine).with_endpoint("/mcp");

    App::new()
        .with_options(|opt| {
            opt.with_name("Axum Example Server")
                .set_http(http)
                .with_mcp_version("2025-06-18")
        })
        .run()
        .await;
}

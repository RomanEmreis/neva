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
//! engine-agnostic `http-server` feature, implements
//! the [`HttpEngine`] contract for an `AxumEngine`, and wires it into
//! `HttpServer::from_engine`. All adapter surfaces — HTTP request /
//! response conversion *and* SSE event construction — live on the
//! engine, so route handlers are one-liners.

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
use neva::prelude::*;
use std::convert::Infallible;
use tokio_util::sync::CancellationToken;

/// HTTP engine backed by [axum](https://docs.rs/axum).
#[derive(Default, Debug)]
struct AxumEngine;

impl HttpEngine for AxumEngine {
    type Request = http::Request<Body>;
    type Response = Response;
    type SseEvent = Result<Event, Infallible>;

    async fn adapt_request(req: Self::Request) -> HttpRequest {
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

    fn adapt_response(resp: HttpResponse) -> Self::Response {
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

    fn tracked_event(seq: u64, msg: &Message) -> Self::SseEvent {
        Ok(Event::default()
            .id(seq.to_string())
            .json_data(msg)
            .unwrap_or_default())
    }

    fn ephemeral_event(msg: &Message) -> Self::SseEvent {
        Ok(Event::default().json_data(msg).unwrap_or_default())
    }

    async fn run(self, ctx: HttpContext, token: CancellationToken) -> Result<(), Error> {
        let addr = ctx.addr().to_owned();
        let endpoint = ctx.endpoint().to_owned();

        let app = Router::new()
            .route(
                &endpoint,
                post(post_handler).get(get_handler).delete(delete_handler),
            )
            .with_state(ctx);

        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .map_err(|e| Error::new(ErrorCode::InternalError, e.to_string()))?;

        axum::serve(listener, app)
            .with_graceful_shutdown(async move { token.cancelled().await })
            .await
            .map_err(|e| Error::new(ErrorCode::InternalError, e.to_string()))
    }
}

async fn post_handler(State(ctx): State<HttpContext>, req: axum::http::Request<Body>) -> Response {
    handlers::dispatch_post::<AxumEngine>(req, &ctx).await
}

async fn delete_handler(State(ctx): State<HttpContext>, req: http::Request<Body>) -> Response {
    handlers::dispatch_delete::<AxumEngine>(req, &ctx).await
}

async fn get_handler(State(ctx): State<HttpContext>, req: http::Request<Body>) -> Response {
    match handlers::dispatch_get_sse::<AxumEngine>(req, &ctx).await {
        SseResponse::Stream { headers, stream } => {
            let sse = Sse::new(stream).keep_alive(KeepAlive::default());
            let mut response: Response = sse.into_response();
            for (name, value) in headers.iter() {
                response.headers_mut().insert(name, value.clone());
            }
            response
        }
        SseResponse::Status(resp) => AxumEngine::adapt_response(resp),
    }
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

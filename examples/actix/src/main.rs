//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector
//!
//! cargo run -p example-actix
//! ```
//!
//! This example shows how to plug a non-default HTTP stack — here, actix-web —
//! into neva's Streamable HTTP transport. It pulls in `neva` with only the
//! engine-agnostic `http-server` feature, implements
//! the [`HttpEngine`] contract for an `ActixEngine`, and wires it into
//! `HttpServer::from_engine`.

use actix_web::{
    App as ActixApp, HttpRequest as ActixHttpRequest, HttpResponse as ActixHttpResponse,
    HttpServer as ActixHttpServer,
    web::{self, Bytes as ActixBytes, Data},
};
use bytes::Bytes;
use neva::prelude::*;
use tokio_util::sync::CancellationToken;

/// HTTP engine backed by [actix-web](https://docs.rs/actix-web).
///
/// `Request` / `Response` are actix's own native types — they're `!Send`
/// but the `HttpEngine` trait doesn't require Send there, and actix
/// handlers never cross a `tokio::spawn` boundary so the `!Send` future
/// produced by `dispatch_*` is fine.
#[derive(Default, Debug)]
struct ActixEngine;

impl HttpEngine for ActixEngine {
    /// actix delivers the request metadata and the buffered body as
    /// separate extractor arguments, so we wrap them in a tuple.
    type Request = (ActixHttpRequest, ActixBytes);
    type Response = ActixHttpResponse;
    /// Pre-formatted SSE wire bytes — actix-web's `streaming(...)`
    /// consumes a stream of `Result<Bytes, _>`, and we wrap each event
    /// in `Ok` because neva never produces SSE errors.
    type SseEvent = Result<Bytes, std::convert::Infallible>;

    async fn adapt_request(req: Self::Request) -> HttpRequest {
        let (req, body) = req;

        let method = http::Method::from_bytes(req.method().as_str().as_bytes())
            .unwrap_or(http::Method::POST);
        let uri = req
            .uri()
            .to_string()
            .parse::<http::Uri>()
            .unwrap_or_default();

        let mut builder = http::Request::builder().method(method).uri(uri);
        if let Some(headers) = builder.headers_mut() {
            for (name, value) in req.headers().iter() {
                if let (Ok(n), Ok(v)) = (
                    http::HeaderName::from_bytes(name.as_str().as_bytes()),
                    http::HeaderValue::from_bytes(value.as_bytes()),
                ) {
                    headers.append(n, v);
                }
            }
        }
        builder
            .body(Bytes::copy_from_slice(&body))
            .expect("valid request")
    }

    fn adapt_response(resp: HttpResponse) -> Self::Response {
        let (parts, body) = resp.into_parts();
        let status = actix_web::http::StatusCode::from_u16(parts.status.as_u16())
            .unwrap_or(actix_web::http::StatusCode::OK);

        let mut builder = ActixHttpResponse::build(status);
        for (name, value) in parts.headers.iter() {
            if let (Ok(n), Ok(v)) = (
                actix_web::http::header::HeaderName::from_bytes(name.as_str().as_bytes()),
                actix_web::http::header::HeaderValue::from_bytes(value.as_bytes()),
            ) {
                builder.append_header((n, v));
            }
        }
        builder.body(body)
    }

    fn tracked_event(seq: u64, msg: &Message) -> Self::SseEvent {
        let payload = serde_json::to_string(msg).unwrap_or_default();
        Ok(Bytes::from(format!("id: {seq}\ndata: {payload}\n\n")))
    }

    fn ephemeral_event(msg: &Message) -> Self::SseEvent {
        let payload = serde_json::to_string(msg).unwrap_or_default();
        Ok(Bytes::from(format!("data: {payload}\n\n")))
    }

    async fn run(self, ctx: HttpContext, token: CancellationToken) -> Result<(), Error> {
        let addr = ctx.addr().to_owned();
        let endpoint = ctx.endpoint().to_owned();

        let server = ActixHttpServer::new(move || {
            let endpoint = endpoint.clone();
            ActixApp::new()
                .app_data(Data::new(ctx.clone()))
                .route(&endpoint, web::post().to(post_handler))
                .route(&endpoint, web::delete().to(delete_handler))
                .route(&endpoint, web::get().to(get_handler))
        })
        .bind(addr.as_str())
        .map_err(|e| Error::new(ErrorCode::InternalError, e.to_string()))?
        .disable_signals()
        .run();

        let handle = server.handle();

        tokio::spawn(async move {
            token.cancelled().await;
            handle.stop(true).await;
        });

        server
            .await
            .map_err(|e| Error::new(ErrorCode::InternalError, e.to_string()))
    }
}

async fn post_handler(
    ctx: Data<HttpContext>,
    req: ActixHttpRequest,
    body: ActixBytes,
) -> ActixHttpResponse {
    handlers::dispatch_post::<ActixEngine>((req, body), &ctx).await
}

async fn delete_handler(
    ctx: Data<HttpContext>,
    req: ActixHttpRequest,
    body: ActixBytes,
) -> ActixHttpResponse {
    handlers::dispatch_delete::<ActixEngine>((req, body), &ctx).await
}

async fn get_handler(
    ctx: Data<HttpContext>,
    req: ActixHttpRequest,
    body: ActixBytes,
) -> ActixHttpResponse {
    match handlers::dispatch_get_sse::<ActixEngine>((req, body), &ctx).await {
        SseResponse::Stream { headers, stream } => {
            let mut builder = ActixHttpResponse::Ok();
            builder.content_type("text/event-stream");
            for (name, value) in headers.iter() {
                if let (Ok(n), Ok(v)) = (
                    actix_web::http::header::HeaderName::from_bytes(name.as_str().as_bytes()),
                    actix_web::http::header::HeaderValue::from_bytes(value.as_bytes()),
                ) {
                    builder.append_header((n, v));
                }
            }
            builder.streaming(stream)
        }
        SseResponse::Status(resp) => ActixEngine::adapt_response(resp),
    }
}

#[tool]
async fn hello(name: String) -> String {
    format!("Hello, {name}!")
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let http = HttpServer::from_engine("127.0.0.1:3000", ActixEngine).with_endpoint("/mcp");

    App::new()
        .with_options(|opt| {
            opt.with_name("Actix Example Server")
                .set_http(http)
                .with_mcp_version("2025-06-18")
        })
        .run()
        .await;
}

//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector
//!
//! cargo run -p example-hyper
//! ```
//!
//! This example shows how to plug a non-default HTTP stack — here, raw
//! hyper — into neva's Streamable HTTP transport. It pulls in `neva`
//! with only the engine-agnostic `http-server` feature, implements the [`HttpEngine`] contract for a `HyperEngine`,
//! and wires it into `HttpServer::from_engine`.
//!
//! Unlike axum / actix-web, hyper ships no router — the engine's
//! accept loop dispatches on `(method, path)` directly.

use std::convert::Infallible;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full, StreamBody, combinators::BoxBody};
use hyper::{
    Method,
    body::{Frame, Incoming},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use neva::prelude::*;
use tokio_util::sync::CancellationToken;

/// Boxed body type used uniformly by every response this engine builds —
/// `BoxBody` lets the SSE-streaming branch and the buffered-JSON branch
/// share a single response type.
type BoxedBody = BoxBody<Bytes, Infallible>;

/// HTTP engine backed by raw [hyper](https://docs.rs/hyper).
///
/// hyper exposes only the protocol layer, so this engine owns the
/// accept loop, per-connection service registration, and the
/// `(method, path)` dispatch table that frameworks normally provide.
#[derive(Default, Debug)]
struct HyperEngine;

impl HttpEngine for HyperEngine {
    type Request = http::Request<Incoming>;
    type Response = http::Response<BoxedBody>;
    // Each SSE event is a `Frame::data(...)` carrying pre-formatted
    // wire bytes; `StreamBody` turns the stream of frames into a
    // `Body` impl we can hand back to hyper.
    type SseEvent = Result<Frame<Bytes>, Infallible>;

    async fn adapt_request(req: Self::Request) -> Result<HttpRequest, Error> {
        let (parts, body) = req.into_parts();
        let bytes = body
            .collect()
            .await
            .map(|c| c.to_bytes())
            .map_err(|e| Error::new(ErrorCode::InternalError, e.to_string()))?;
        Ok(http::Request::from_parts(parts, bytes))
    }

    fn adapt_response(resp: HttpResponse) -> Self::Response {
        let (parts, body) = resp.into_parts();
        let boxed = Full::new(body)
            .map_err(|never: Infallible| match never {})
            .boxed();
        http::Response::from_parts(parts, boxed)
    }

    fn tracked_event(seq: u64, msg: &Message) -> Self::SseEvent {
        let payload = serde_json::to_string(msg).unwrap_or_default();
        Ok(Frame::data(Bytes::from(format!(
            "id: {seq}\ndata: {payload}\n\n"
        ))))
    }

    fn ephemeral_event(msg: &Message) -> Self::SseEvent {
        let payload = serde_json::to_string(msg).unwrap_or_default();
        Ok(Frame::data(Bytes::from(format!("data: {payload}\n\n"))))
    }

    async fn run(self, ctx: HttpContext, token: CancellationToken) -> Result<(), Error> {
        let addr = ctx.addr().to_owned();
        let listener = tokio::net::TcpListener::bind(addr.as_str())
            .await
            .map_err(|e| Error::new(ErrorCode::InternalError, e.to_string()))?;

        loop {
            tokio::select! {
                _ = token.cancelled() => return Ok(()),
                accept_res = listener.accept() => {
                    let Ok((stream, _peer)) = accept_res else { continue };
                    let ctx = ctx.clone();
                    tokio::spawn(async move {
                        let io = TokioIo::new(stream);
                        let service = service_fn(move |req: http::Request<Incoming>| {
                            let ctx = ctx.clone();
                            async move {
                                Ok::<_, Infallible>(dispatch(req, ctx).await)
                            }
                        });

                        if let Err(e) = http1::Builder::new()
                            .serve_connection(io, service)
                            .await
                        {
                            tracing::debug!(?e, "hyper connection error");
                        }
                    });
                }
            }
        }
    }
}

/// Per-request dispatch: gate on the configured endpoint path and method.
async fn dispatch(req: http::Request<Incoming>, ctx: HttpContext) -> http::Response<BoxedBody> {
    if req.uri().path() != ctx.endpoint() {
        return status_only(http::StatusCode::NOT_FOUND);
    }
    match *req.method() {
        Method::POST => handlers::dispatch_post::<HyperEngine>(req, &ctx)
            .await
            .unwrap_or_else(|_| status_only(http::StatusCode::INTERNAL_SERVER_ERROR)),
        Method::DELETE => handlers::dispatch_delete::<HyperEngine>(req, &ctx)
            .await
            .unwrap_or_else(|_| status_only(http::StatusCode::INTERNAL_SERVER_ERROR)),
        Method::GET => {
            let outcome = match handlers::dispatch_get_sse::<HyperEngine>(req, &ctx).await {
                Ok(outcome) => outcome,
                Err(_) => return status_only(http::StatusCode::INTERNAL_SERVER_ERROR),
            };
            match outcome {
                SseResponse::Stream { headers, stream } => {
                    let body = StreamBody::new(stream).boxed();
                    let mut resp = http::Response::builder()
                        .status(http::StatusCode::OK)
                        .header(http::header::CONTENT_TYPE, "text/event-stream")
                        .body(body)
                        .expect("valid response");
                    for (name, value) in headers.iter() {
                        resp.headers_mut().insert(name, value.clone());
                    }
                    resp
                }
                SseResponse::Status(resp) => HyperEngine::adapt_response(resp),
            }
        }
        _ => status_only(http::StatusCode::METHOD_NOT_ALLOWED),
    }
}

fn status_only(status: http::StatusCode) -> http::Response<BoxedBody> {
    let body = Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed();
    http::Response::builder()
        .status(status)
        .body(body)
        .expect("valid response")
}

#[tool]
async fn hello(name: String) -> String {
    format!("Hello, {name}!")
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let http = HttpServer::from_engine("127.0.0.1:3000", HyperEngine).with_endpoint("/mcp");

    App::new()
        .with_options(|opt| {
            opt.with_name("Hyper Example Server")
                .set_http(http)
                .with_mcp_version("2025-06-18")
        })
        .run()
        .await;
}

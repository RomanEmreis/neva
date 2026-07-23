//! A custom [`HttpEngine`] on bare hyper - serving a
//! protected MCP server with the engine-neutral OAuth primitives:
//!
//! * the RFC 9728 Protected Resource Metadata document is mounted on
//!   [`HttpContext::oauth_metadata_path`] and served by
//!   [`handlers::handle_oauth_metadata`];
//! * requests failing token validation answer with
//!   [`handlers::handle_unauthorized`], so the `401` carries the
//!   `WWW-Authenticate: Bearer resource_metadata="..."` challenge;
//! * decoded claims go into the neutral request's extensions as
//!   `Arc<dyn Claims>`, which keeps `#[tool(roles = [...])]` gates
//!   working exactly like under the default Volga engine.
//!
//! Token validation here is HS256 with a shared secret to keep the
//! example self-contained -- swap `decode_claims` for your JWKS-based
//! validation against a real issuer.
//!
//! Run with:
//!
//! ```no_rust
//! JWT_SECRET=a-string-secret-at-least-256-bits-long \
//!     cargo run -p example-oauth-hyper-engine
//! ```
use bytes::Bytes;
use futures_util::StreamExt;
use http_body_util::{BodyExt, Full, StreamBody, combinators::BoxBody};
use hyper::body::{Frame, Incoming};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use neva::error::{Error, ErrorCode};
use neva::prelude::*;
use neva::types::Message;

/// A tool available to any authenticated caller
#[tool]
async fn whoami() -> &'static str {
    "an authenticated caller, served by hyper"
}

/// A tool available only to admins
#[tool(roles = ["admin"])]
async fn admin_report(name: String) -> String {
    format!("confidential report: {name}")
}

struct HyperEngine {
    secret: Arc<str>,
}

impl HttpEngine for HyperEngine {
    type Request = http::Request<Incoming>;
    type Response = http::Response<BoxBody<Bytes, Infallible>>;
    /// Pre-rendered SSE frames.
    type SseEvent = Bytes;

    async fn adapt_request(req: Self::Request) -> Result<HttpRequest, Error> {
        let (parts, body) = req.into_parts();
        let body = body
            .collect()
            .await
            .map_err(|err| Error::new(ErrorCode::InternalError, err.to_string()))?
            .to_bytes();
        Ok(HttpRequest::from_parts(parts, body))
    }

    fn adapt_response(resp: HttpResponse) -> Self::Response {
        resp.map(|body| Full::new(body).boxed())
    }

    fn tracked_event(seq: u64, msg: &Message) -> Self::SseEvent {
        let json = serde_json::to_string(msg).unwrap_or_default();
        Bytes::from(format!("id: {seq}\ndata: {json}\n\n"))
    }

    fn ephemeral_event(msg: &Message) -> Self::SseEvent {
        let json = serde_json::to_string(msg).unwrap_or_default();
        Bytes::from(format!("data: {json}\n\n"))
    }

    async fn run(self, ctx: HttpContext, token: CancellationToken) -> Result<(), Error> {
        let listener = TcpListener::bind(ctx.addr()).await.map_err(Error::from)?;
        let ctx = Arc::new(ctx);
        let secret = self.secret;

        loop {
            tokio::select! {
                _ = token.cancelled() => return Ok(()),
                conn = listener.accept() => {
                    let (stream, _) = conn.map_err(Error::from)?;
                    let ctx = ctx.clone();
                    let secret = secret.clone();
                    tokio::spawn(async move {
                        let service = service_fn(move |req| {
                            route(req, ctx.clone(), secret.clone())
                        });
                        let _ = hyper::server::conn::http1::Builder::new()
                            .serve_connection(TokioIo::new(stream), service)
                            .await;
                    });
                }
            }
        }
    }
}

/// The engine's router -- the hyper counterpart of the three MCP routes
/// plus the well-known metadata document.
async fn route(
    req: http::Request<Incoming>,
    ctx: Arc<HttpContext>,
    secret: Arc<str>,
) -> Result<http::Response<BoxBody<Bytes, Infallible>>, Infallible> {
    let path = req.uri().path().to_owned();
    let method = req.method().clone();

    // RFC 9728: publicly reachable, no token required.
    if method == http::Method::GET && Some(path.as_str()) == ctx.oauth_metadata_path() {
        return Ok(HyperEngine::adapt_response(
            handlers::handle_oauth_metadata(&ctx),
        ));
    }

    if path != ctx.endpoint() {
        return Ok(status(http::StatusCode::NOT_FOUND));
    }

    let Ok(mut neutral) = HyperEngine::adapt_request(req).await else {
        return Ok(status(http::StatusCode::INTERNAL_SERVER_ERROR));
    };

    // The engine authorization contract: validate the credential, put
    // the claims into the neutral request's extensions -- or answer with
    // the challenge so the client can start the OAuth flow.
    match decode_claims(neutral.headers(), &ctx, &secret) {
        Some(claims) => {
            let claims: Arc<dyn Claims> = Arc::new(claims);
            neutral.extensions_mut().insert(claims);
        }
        None => {
            return Ok(HyperEngine::adapt_response(handlers::handle_unauthorized(
                &ctx,
            )));
        }
    }

    let resp = match method {
        http::Method::POST => {
            HyperEngine::adapt_response(handlers::handle_post(neutral, &ctx).await)
        }
        http::Method::DELETE => {
            HyperEngine::adapt_response(handlers::handle_delete(neutral, &ctx).await)
        }
        http::Method::GET => match handlers::handle_get_sse::<HyperEngine>(neutral, &ctx).await {
            SseResponse::Status(resp) => HyperEngine::adapt_response(resp),
            SseResponse::Stream { headers, stream } => {
                let body = StreamBody::new(stream.map(|event| Ok(Frame::data(event))));
                let mut resp = http::Response::builder()
                    .status(http::StatusCode::OK)
                    .header(http::header::CONTENT_TYPE, "text/event-stream")
                    .header(http::header::CACHE_CONTROL, "no-cache")
                    .body(BodyExt::boxed(body))
                    .unwrap_or_default();
                resp.headers_mut().extend(headers);
                resp
            }
        },
        _ => status(http::StatusCode::METHOD_NOT_ALLOWED),
    };
    Ok(resp)
}

/// HS256 bearer validation with the audience bound to the canonical
/// resource URI -- the place to plug JWKS validation instead.
fn decode_claims(
    headers: &http::HeaderMap,
    ctx: &HttpContext,
    secret: &str,
) -> Option<DefaultClaims> {
    let token = headers
        .get(http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")?;

    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
    if let Some(resource) = ctx.oauth_resource() {
        validation.set_audience(&[resource]);
    }

    jsonwebtoken::decode::<DefaultClaims>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .ok()
    .map(|data| data.claims)
}

fn status(code: http::StatusCode) -> http::Response<BoxBody<Bytes, Infallible>> {
    http::Response::builder()
        .status(code)
        .body(Full::new(Bytes::new()).boxed())
        .unwrap_or_default()
}

#[tokio::main]
async fn main() {
    let secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");
    let engine = HyperEngine {
        secret: secret.into(),
    };

    App::new()
        .with_options(|opt| {
            opt.with_name("Hyper-engine OAuth Example").set_http(
                HttpServer::from_engine("127.0.0.1:3005", engine).with_oauth_metadata(|oauth| {
                    oauth
                        .with_authorization_servers(["https://auth.example.com"])
                        .with_scopes(["mcp:tools"])
                }),
            )
        })
        .run()
        .await;
}

//! [`VolgaEngine`] -- the default [`HttpEngine`] implementation.
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
    types::{
        DefaultClaims, EventId, HttpRequest as NeutralRequest, HttpResponse as NeutralResponse,
    },
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
/// // wired into `HttpServer` by Task 13 -- engines never run standalone.
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
        // Claims decoded and validated by Volga's `authorize` middleware --
        // the single decode path for both static-key and OAuth/JWKS modes.
        // Reading them from the request (rather than re-decoding the
        // `Authorization` header) also survives Volga's default
        // `strip_token_from_request`, which removes the header before the
        // route runs. Inserting them here (per the `HttpEngine` contract)
        // keeps the Volga routes on the same `dispatch_*` seam as every
        // other engine.
        let claims: Option<::volga::auth::Authenticated<DefaultClaims>> = req.extract().ok();

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

        let mut neutral = builder
            .body(body)
            .map_err(|e| Error::new(ErrorCode::InternalError, e.to_string()))?;

        if let Some(claims) = claims {
            let claims: Arc<dyn crate::auth::Claims> = Arc::new(claims.into_inner());
            neutral.extensions_mut().insert(claims);
        }

        Ok(neutral)
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

    fn tracked_event(id: EventId, msg: &Message) -> Self::SseEvent {
        SseMessage::new().id(id.to_string()).json(msg)
    }

    fn ephemeral_event(msg: &Message) -> Self::SseEvent {
        SseMessage::new().json(msg)
    }

    async fn run(self, ctx: HttpContext, token: CancellationToken) -> Result<(), Error> {
        let addr = ctx.addr().to_owned();
        let endpoint = ctx.endpoint().to_owned();
        #[cfg(feature = "server-oauth")]
        let oauth_metadata_path = ctx.oauth_metadata_path().map(str::to_owned);
        #[cfg(feature = "server-oauth")]
        let oauth_metadata_url = ctx.oauth_metadata_url().map(str::to_owned);

        // The transport's token is what stops this server, as the
        // `HttpEngine::run` contract asks: it composes with Volga's own signal
        // handling rather than replacing it, so Ctrl+C still works, and a
        // programmatic `ShutdownHandle` now takes the listener down with it
        // instead of leaving it bound and serving. Volga's graceful shutdown
        // keeps in-flight connections -- the response body a subscription is
        // written onto among them -- open while it drains.
        let shutdown = token.clone();
        let mut server = App::new()
            .bind(addr.as_str())
            .with_no_delay()
            .without_greeter()
            .shutdown_on(async move { shutdown.cancelled().await });

        let rules = match self.auth {
            Some(auth) => {
                #[cfg(feature = "server-oauth")]
                let mut auth = auth;
                // In OAuth issuer mode, default the token checks to the
                // MCP contract: `aud` must contain the canonical resource
                // URI (RFC 8707) and `iss` must match the issuer.
                #[cfg(feature = "server-oauth")]
                auth.apply_mcp_defaults(ctx.oauth_resource());
                // `with_oauth` without an issuer is a config error --
                // surface it as a failed start, not a Volga panic.
                #[cfg(feature = "server-oauth")]
                let volga_oauth = auth.take_oauth()?;

                let (bearer, rules) = auth.into_parts();
                // Advertise the RFC 9728 document on Volga's own 401
                // challenges: `WWW-Authenticate: Bearer resource_metadata="..."`.
                #[cfg(feature = "server-oauth")]
                let bearer = match &oauth_metadata_url {
                    Some(url) => bearer.with_resource_metadata_url(url.as_str()),
                    None => bearer,
                };
                server = server.with_bearer_auth(|_| bearer);

                // Issuer-based JWKS validation: keys are discovered from
                // the issuer's metadata and rotated by Volga.
                #[cfg(feature = "server-oauth")]
                if let Some(oauth) = volga_oauth {
                    server = server.with_oauth(|_| oauth);
                    server.use_oauth();
                }
                Some(rules)
            }
            None => None,
        };

        #[cfg(feature = "server-tls")]
        if let Some(tls) = self.tls {
            server = server.set_tls(tls);
        }

        server
            .add_singleton(ctx)
            .map_err(handle_http_error)
            .group(endpoint.as_str(), move |mcp| {
                // Token validation and role/permission rules guard only
                // the MCP endpoint group; the well-known metadata route
                // below stays outside it.
                if let Some(rules) = rules {
                    mcp.authorize(rules);
                }
                mcp.map_post("/", routes::post);
                // Stateless 2026-07-28 transport has no SSE GET stream and no
                // session-termination DELETE -- only POST is routed.
                #[cfg(feature = "legacy-spec")]
                {
                    mcp.map_get("/", routes::get);
                    mcp.map_delete("/", routes::delete);
                }
            });

        // RFC 9728 section 3: the Protected Resource Metadata document must be
        // reachable without credentials.
        #[cfg(feature = "server-oauth")]
        if let Some(path) = &oauth_metadata_path {
            server.map_get(path, routes::oauth_metadata);
        }

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

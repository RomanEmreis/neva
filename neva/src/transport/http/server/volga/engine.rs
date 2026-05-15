//! [`VolgaEngine`] — the default [`HttpEngine`] implementation.
//!
//! This engine is bound by `HttpServer` when the `http-server-volga`
//! feature is enabled. It owns the Volga adapter logic exclusively: any
//! engine-agnostic JSON-RPC / SSE behavior lives in
//! [`crate::transport::http::core`].

use super::auth_config::AuthConfig;
use crate::error::{Error, ErrorCode};
use crate::transport::http::core::{context::HttpContext, engine::HttpEngine};
use ::volga::App;
#[cfg(feature = "server-tls")]
use ::volga::tls::TlsConfig;
use std::{future::Future, sync::Arc};
use tokio_util::sync::CancellationToken;

use super::responder::VolgaSseResponder;
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
    type SseResponder = VolgaSseResponder;

    #[allow(clippy::manual_async_fn)]
    fn run(
        self,
        ctx: Arc<HttpContext>,
        token: CancellationToken,
    ) -> impl Future<Output = Result<(), Error>> + Send {
        async move {
            let addr = ctx.addr();
            let endpoint = ctx.endpoint();

            let mut server = App::new().bind(addr).with_no_delay().without_greeter();

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
                .add_singleton(ctx.clone())
                .map_err(handle_http_error)
                .group(endpoint, |mcp| {
                    mcp.map_post("/", routes::post);
                    mcp.map_get("/", routes::get);
                    mcp.map_delete("/", routes::delete);
                });

            if let Err(e) = server.run().await {
                token.cancel();
                return Err(Error::new(ErrorCode::InternalError, e.to_string()));
            }
            Ok(())
        }
    }
}

async fn handle_http_error(_err: ::volga::error::Error) {
    #[cfg(feature = "tracing")]
    tracing::error!(logger = "neva", "HTTP error: {:?}", _err);
}

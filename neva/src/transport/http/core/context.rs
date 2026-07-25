//! Per-server shared state passed to [`HttpEngine`](super::engine::HttpEngine).

use crate::{error::Error, shared::SseSessionRegistry, types::Message};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// Map from `(session_id, request_id)` to the oneshot waiting for the
/// matching response. Keyed by `Message::full_id`.
pub(crate) type RequestMap = Arc<DashMap<crate::types::RequestId, oneshot::Sender<Message>>>;

/// Per-server context handed to an engine's `run` method.
///
/// Holds everything the engine needs to wire its three MCP routes:
/// the bound address, the endpoint prefix, the pending-request map,
/// the SSE session registry, and per-session queue capacities.
///
/// All fields are cheaply cloneable (Arc / Copy), so engines can move
/// the whole context into route handlers or register it in the
/// framework's state/DI as-is -- an extra `Arc` wrap is never needed
/// (Volga's `Dc<T>` and actix's `Data<T>` already add their own).
///
/// Fields are `pub(crate)`; engines interact through the public
/// accessors and the helpers in [`super::handlers`].
#[derive(Clone, Debug)]
pub struct HttpContext {
    pub(crate) addr: Arc<str>,
    pub(crate) endpoint: Arc<str>,
    pub(crate) pending: RequestMap,
    pub(crate) sse_registry: Arc<SseSessionRegistry>,
    pub(crate) inbound_tx: mpsc::Sender<Result<Message, Error>>,
    pub(crate) sse_live_queue_capacity: usize,
    pub(crate) sse_log_queue_capacity: usize,
    #[cfg(feature = "server-oauth")]
    pub(crate) oauth: Option<super::oauth::OAuthResource>,
}

impl HttpContext {
    /// The address this server is bound to (e.g. `"127.0.0.1:3000"`).
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// The MCP endpoint prefix (e.g. `"/mcp"`).
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The path of the RFC 9728 Protected Resource Metadata document
    /// (e.g. `"/.well-known/oauth-protected-resource/mcp"`), when OAuth
    /// is configured via
    /// [`HttpServer::with_oauth_metadata`](crate::transport::http::HttpServer::with_oauth_metadata).
    ///
    /// An engine mounts a GET route here and serves it with
    /// [`handlers::handle_oauth_metadata`](super::handlers::handle_oauth_metadata).
    #[cfg(feature = "server-oauth")]
    pub fn oauth_metadata_path(&self) -> Option<&str> {
        self.oauth.as_ref().map(|o| &*o.metadata_path)
    }

    /// The absolute URL of the RFC 9728 Protected Resource Metadata
    /// document (e.g.
    /// `"https://api.example.com/.well-known/oauth-protected-resource/mcp"`),
    /// when OAuth is configured.
    ///
    /// [`handlers::handle_unauthorized`](super::handlers::handle_unauthorized)
    /// already advertises it on 401s; an engine that emits its own
    /// `WWW-Authenticate` challenge (through a framework bearer-auth
    /// pipeline) uses this as the `resource_metadata` parameter.
    #[cfg(feature = "server-oauth")]
    pub fn oauth_metadata_url(&self) -> Option<&str> {
        self.oauth.as_ref().map(|o| &*o.metadata_url)
    }

    /// The canonicalized resource identifier (RFC 8707) this server is
    /// deployed as (e.g. `"https://api.example.com/mcp"`), when OAuth is
    /// configured.
    ///
    /// This is the audience value access tokens must be bound to -- the
    /// default Volga adapter feeds it into bearer validation as a
    /// required `aud`; a custom engine should enforce the same check.
    #[cfg(feature = "server-oauth")]
    pub fn oauth_resource(&self) -> Option<&str> {
        self.oauth.as_ref().map(|o| &*o.resource)
    }
}

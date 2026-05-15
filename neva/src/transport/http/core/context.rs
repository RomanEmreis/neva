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
/// Engines treat this type as opaque — its fields are `pub(crate)`.
/// The only operations engines need are reading `addr`/`endpoint`,
/// cloning the `Arc<HttpContext>` into route handlers, and passing
/// `&HttpContext` to the [`handlers`](super::handlers) helpers.
#[derive(Debug)]
pub struct HttpContext {
    pub(crate) addr: &'static str,
    pub(crate) endpoint: &'static str,
    pub(crate) pending: RequestMap,
    pub(crate) sse_registry: Arc<SseSessionRegistry>,
    pub(crate) inbound_tx: mpsc::Sender<Result<Message, Error>>,
    pub(crate) sse_live_queue_capacity: usize,
    pub(crate) sse_log_queue_capacity: usize,
}

impl HttpContext {
    /// The address this server is bound to (e.g. `"127.0.0.1:3000"`).
    pub fn addr(&self) -> &'static str {
        self.addr
    }

    /// The MCP endpoint prefix (e.g. `"/mcp"`).
    pub fn endpoint(&self) -> &'static str {
        self.endpoint
    }
}

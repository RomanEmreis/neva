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
/// the whole context into route handlers — wrap it in `Arc` only if
/// the framework's state pattern requires that (Volga `add_singleton`,
/// axum `with_state`).
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
}

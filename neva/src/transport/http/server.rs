//! HTTP server implementation

use super::{HttpRuntimeContext, MCP_SESSION_ID, ServiceUrl, get_mcp_session_id};
#[cfg(feature = "tracing")]
use crate::types::notification::fmt::LOG_REGISTRY;
use crate::{
    error::{Error, ErrorCode},
    shared::SseSessionRegistry,
    types::{Message, RequestId, Response},
};
use dashmap::DashMap;
use futures_util::{Stream, future::Either, stream};
use std::{pin::Pin, sync::Arc};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::{StreamExt, wrappers::ReceiverStream};
use tokio_util::sync::CancellationToken;
#[cfg(feature = "server-tls")]
use volga::tls::TlsConfig;
use volga::{
    App, HttpRequest, HttpResult,
    auth::{Bearer, BearerTokenService},
    di::Dc,
    headers::{AUTHORIZATION, HeaderMap},
    http::sse::Message as SseMessage,
    ok, sse, status,
};

pub use auth_config::{AuthConfig, DefaultClaims};
pub(crate) use auth_config::{validate_permissions, validate_roles};

pub(crate) mod auth_config;

type RequestMap = Arc<DashMap<RequestId, oneshot::Sender<Message>>>;

/// Unified SSE stream item type.
///
/// `Tracked` events carry a sequence number and are buffered for replay.
/// `Ephemeral` events (tracing log notifications) have no `id:` field — they do not
/// advance the client's Last-Event-ID and are never replayed.
enum SseItem {
    /// MCP protocol message: carries a seq number, buffered for Last-Event-ID replay.
    Tracked(u64, Arc<Message>),
    /// Tracing notification: ephemeral, emitted without an SSE `id:` field.
    Ephemeral(Box<Message>),
}

#[derive(Clone)]
struct RequestManager {
    pending: RequestMap,
    sse_registry: Arc<SseSessionRegistry>,
    sender: mpsc::Sender<Result<Message, Error>>,
    sse_live_queue_capacity: usize,
    sse_log_queue_capacity: usize,
}

struct SseConnectionCleanup {
    id: uuid::Uuid,
    generation: u64,
    registry: Arc<SseSessionRegistry>,
}

impl Drop for SseConnectionCleanup {
    fn drop(&mut self) {
        #[cfg(feature = "tracing")]
        LOG_REGISTRY.unregister(&self.id);
        self.registry.unregister(&self.id, self.generation);
    }
}

pub(super) async fn serve(rt: HttpRuntimeContext, token: CancellationToken) {
    let pending = Arc::new(DashMap::new());
    let sse_registry = Arc::new(SseSessionRegistry::new(rt.sse_buffer_capacity));
    let manager = RequestManager {
        pending: pending.clone(),
        sse_registry: sse_registry.clone(),
        sender: rt.tx,
        sse_live_queue_capacity: rt.sse_live_queue_capacity,
        sse_log_queue_capacity: rt.sse_log_queue_capacity,
    };
    tokio::join!(
        dispatch(pending.clone(), sse_registry.clone(), rt.rx, token.clone()),
        cleanup_stale_sessions(
            sse_registry.clone(),
            rt.sse_cleanup_interval,
            rt.sse_session_ttl,
            token.clone()
        ),
        handle(
            rt.url,
            rt.auth,
            #[cfg(feature = "server-tls")]
            rt.tls_config,
            manager,
            token.clone()
        )
    );
}

async fn dispatch(
    pending: RequestMap,
    sse_registry: Arc<SseSessionRegistry>,
    mut sender_rx: mpsc::Receiver<Message>,
    token: CancellationToken,
) {
    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => break,
            Some(msg) = sender_rx.recv() => {
                if let Some((_, resp_tx)) = pending.remove(&msg.full_id()) {
                    if let Err(_e) = resp_tx.send(msg) {
                        #[cfg(feature = "tracing")]
                        tracing::error!(logger = "neva", "Failed to send response: {:?}", _e);
                        token.cancel();
                    }
                } else if let Err(_e) = sse_registry.send(msg) {
                    #[cfg(feature = "tracing")]
                    tracing::error!(logger = "neva", "Failed to send server request: {:?}", _e);
                }
            }
        }
    }
}

async fn cleanup_stale_sessions(
    sse_registry: Arc<SseSessionRegistry>,
    interval: std::time::Duration,
    ttl: std::time::Duration,
    token: CancellationToken,
) {
    let mut ticker = tokio::time::interval(interval);

    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => break,
            _ = ticker.tick() => sse_registry.evict_stale(ttl),
        }
    }
}

async fn handle(
    service_url: ServiceUrl,
    auth: Option<AuthConfig>,
    #[cfg(feature = "server-tls")] tls: Option<TlsConfig>,
    manager: RequestManager,
    token: CancellationToken,
) {
    let root = "/";
    let mut server = App::new()
        .bind(service_url.addr)
        .with_no_delay()
        .without_greeter();

    if let Some(auth) = auth {
        let (auth, rules) = auth.into_parts();
        server = server.with_bearer_auth(|_| auth);
        server.authorize(rules);
    }

    #[cfg(feature = "server-tls")]
    if let Some(tls) = tls {
        server = server.set_tls(tls);
    }

    server
        .add_singleton(manager)
        .map_err(handle_http_error)
        .group(service_url.endpoint, |mcp| {
            mcp.map_get(root, handle_connection);
            mcp.map_post(root, handle_message);
            mcp.map_delete(root, handle_session_end);
        });

    if let Err(_e) = server.run().await {
        #[cfg(feature = "tracing")]
        tracing::error!(logger = "neva", "HTTP Server was shutdown: {:?}", _e);
        token.cancel();
    }
}

async fn handle_session_end(req: HttpRequest) -> HttpResult {
    let Some(id) = get_mcp_session_id(req.headers()) else {
        return status!(400);
    };

    let manager: Dc<RequestManager> = req.extract()?;

    #[cfg(feature = "tracing")]
    LOG_REGISTRY.unregister(&id);
    manager.sse_registry.terminate(&id);

    ok!([(MCP_SESSION_ID, id.to_string())])
}

async fn handle_connection(req: HttpRequest) -> HttpResult {
    let Some(id) = get_mcp_session_id(req.headers()) else {
        return status!(400);
    };

    let manager: Dc<RequestManager> = req.extract()?;

    // Create a typed msg channel and untyped log channel
    let (msg_tx, msg_rx) = mpsc::channel::<(u64, Arc<Message>)>(manager.sse_live_queue_capacity);
    let (_log_tx, log_rx) = mpsc::channel::<Message>(manager.sse_log_queue_capacity);

    // Register log channel (tracing — unchanged)
    #[cfg(feature = "tracing")]
    LOG_REGISTRY.register(id, _log_tx);

    // Register msg channel — updates sender/generation in place for reconnects,
    // preserving buffer and next_seq. The returned generation is intentionally unused here:
    // handle_session_end uses `terminate()` (unconditional removal) rather than the
    // generation-protected `unregister()`, so threading generation to that handler is not needed.
    let generation = manager.sse_registry.register(id, msg_tx);

    // Parse Last-Event-ID header
    let last_seq: Option<u64> = req
        .headers()
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok());

    // Build msg_stream: replay buffered events first, then live.
    //
    // With Last-Event-ID: reconnect path — replay events after last_seq.
    // Without Last-Event-ID: initial connection — replay_all recovers events buffered
    // during the POST → GET handshake window (pre_register in handle_message ensures
    // the buffer exists). If the buffer is empty, fall through to pure live stream.
    let replay = match last_seq {
        Some(seq) => manager.sse_registry.replay_since(&id, seq),
        None => manager.sse_registry.replay_all(&id),
    };

    let msg_stream = if replay.is_empty() {
        Either::Left(ReceiverStream::new(msg_rx).map(|(seq, arc)| SseItem::Tracked(seq, arc)))
    } else {
        // Deduplicate: live stream must not re-emit events already in replay.
        let replay_end_seq = replay.last().map(|(s, _)| *s).unwrap_or(0);
        let replay_stream = stream::iter(replay).map(|(seq, arc)| SseItem::Tracked(seq, arc));
        let live = ReceiverStream::new(msg_rx)
            .filter(move |&(seq, _)| seq > replay_end_seq)
            .map(|(seq, arc)| SseItem::Tracked(seq, arc));
        Either::Right(replay_stream.chain(live))
    };

    // Log stream (ephemeral, no id: field)
    let log_stream = ReceiverStream::new(log_rx).map(|m| SseItem::Ephemeral(Box::new(m)));

    // Merge and stream
    let mut merged = stream::select(log_stream, msg_stream);
    let cleanup = SseConnectionCleanup {
        id,
        generation,
        registry: manager.sse_registry.clone(),
    };
    let guarded = stream::poll_fn(move |cx| {
        let _cleanup = &cleanup;
        Pin::new(&mut merged).poll_next(cx)
    });

    sse!(guarded.map(handle_sse_message); [
        (MCP_SESSION_ID, id.to_string())
    ])
}

async fn handle_message(req: HttpRequest) -> HttpResult {
    let bts: Option<BearerTokenService> = req.extract()?;
    let manager: Dc<RequestManager> = req.extract()?;

    let mut headers = req.headers().clone();
    let id = get_or_create_mcp_session(&headers);

    // JSON-RPC 2.0 §5.1 / §6: decoding failures must be returned as a JSON-RPC
    // error object, not as a transport-level error. read_message distinguishes
    // the two cases required by the spec.
    let msg = match read_message(req).await {
        Ok(msg) => msg,
        Err(code) => {
            let resp = Response::error(RequestId::Null, Error::from(code));
            return ok!(resp; [(MCP_SESSION_ID, id.to_string())]);
        }
    };

    // Pre-register only on the initialize handshake: the server may emit events
    // between the init POST response and the client's SSE GET. Scoped to init so
    // subsequent tool-call POSTs (where SSE is already open) do not create entries
    // for sessions that may never establish or close an SSE channel.
    if let Message::Request(ref r) = msg
        && r.method == crate::commands::INIT
    {
        manager.sse_registry.pre_register(id);
    }

    if matches!(msg, Message::Notification(_)) {
        // JSON-RPC 2.0 §4 / MCP Streamable HTTP: respond 202 immediately,
        // but still forward the notification to the app so it can act on it
        // (e.g. cancel a pending request on notifications/cancelled).
        let msg = msg.set_session_id(id);
        let _ = manager.sender.send(Ok(msg)).await;
        return status!(202; [
            (MCP_SESSION_ID, id.to_string())
        ]);
    }

    // A batch whose items are all notifications/responses produces no reply
    // per JSON-RPC 2.0 #6. Return 202 immediately without allocating a
    // pending entry — otherwise the oneshot receiver would hang forever.
    // session_id must still be set so Response envelopes inside the batch
    // resolve pending entries by the full session_id/request_id key.
    if let Message::Batch(ref batch) = msg
        && !batch.has_requests()
        && !batch.has_error_responses()
    {
        let msg = msg.set_session_id(id);
        manager.sender.send(Ok(msg)).await.map_err(sender_error)?;
        return status!(202; [
            (MCP_SESSION_ID, id.to_string())
        ]);
    }

    let claims = bts
        .and_then(|bts| {
            headers
                .get(AUTHORIZATION)
                .and_then(|bearer| Bearer::try_from(bearer).ok())
                .and_then(|bearer| bts.decode::<DefaultClaims>(bearer).ok())
        })
        .unwrap_or_default();

    headers.remove(AUTHORIZATION);

    let msg = msg
        .set_session_id(id)
        .set_claims(claims)
        .set_headers(headers);

    let (resp_tx, resp_rx) = oneshot::channel::<Message>();
    manager.pending.insert(msg.full_id(), resp_tx);
    manager.sender.send(Ok(msg)).await.map_err(sender_error)?;
    let resp = resp_rx.await.map_err(receiver_error)?;

    ok!(resp; [
        (MCP_SESSION_ID, id.to_string())
    ])
}

/// Reads and decodes a JSON-RPC [`Message`] from the HTTP request body.
///
/// Returns [`ErrorCode`] on failure so the caller can build a spec-compliant
/// error response without promoting the failure to an HTTP-level error:
/// - [`ErrorCode::ParseError`] (`-32700`) — body is not syntactically valid JSON,
///   or the underlying transport stream fails mid-read.
/// - [`ErrorCode::InvalidRequest`] (`-32600`) — body is valid JSON but not a
///   valid JSON-RPC message (e.g. an empty batch `[]` or an unrecognised shape).
#[inline]
async fn read_message(req: HttpRequest) -> Result<Message, ErrorCode> {
    let mut body_data_stream = req.into_body().into_data_stream();
    let mut buf = bytes::BytesMut::new();

    while let Some(chunk) = body_data_stream.next().await {
        let chunk = chunk.map_err(|_| ErrorCode::ParseError)?;
        buf.extend_from_slice(&chunk);
    }

    // Two-step decode per JSON-RPC 2.0 §5.1:
    //   1. Validate JSON syntax → ParseError on failure.
    //   2. Validate message shape → InvalidRequest on failure.
    let value: serde_json::Value =
        serde_json::from_slice(&buf).map_err(|_| ErrorCode::ParseError)?;

    serde_json::from_value::<Message>(value).map_err(|_| ErrorCode::InvalidRequest)
}

async fn handle_http_error(_err: volga::error::Error) {
    #[cfg(feature = "tracing")]
    tracing::error!(logger = "neva", "HTTP error: {:?}", _err)
}

fn sender_error(err: mpsc::error::SendError<Result<Message, Error>>) -> volga::error::Error {
    volga::error::Error::new("/", err.to_string())
}

fn receiver_error(err: oneshot::error::RecvError) -> volga::error::Error {
    volga::error::Error::new("/", err.to_string())
}

fn handle_sse_message(item: SseItem) -> Result<SseMessage, volga::error::Error> {
    match item {
        SseItem::Tracked(seq, msg) => Ok(SseMessage::new().id(seq.to_string()).json(&*msg)),
        SseItem::Ephemeral(msg) => Ok(SseMessage::new().json(*msg)),
    }
}

/// Fetches the [`MCP_SESSION_ID`] header value or created the new [`uuid::Uuid`] if `None`
#[inline]
fn get_or_create_mcp_session(headers: &HeaderMap) -> uuid::Uuid {
    get_mcp_session_id(headers).unwrap_or_else(uuid::Uuid::new_v4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::notification::Notification;

    fn make_notification() -> Message {
        Message::Notification(Notification::new("test", None))
    }

    #[test]
    fn it_emits_id_field_for_tracked_item() {
        let arc = Arc::new(make_notification());
        let item = SseItem::Tracked(42, arc);
        let sse = handle_sse_message(item).expect("should succeed");
        // SseMessage derives Debug; its internal representation stores fields as
        // SseField { bytes: "id: 42\n" }, so the Debug output contains "42".
        let debug = format!("{:?}", sse);
        assert!(
            debug.contains("42"),
            "SSE id field must contain the seq number"
        );
    }

    #[test]
    fn it_does_not_emit_id_field_for_ephemeral_item() {
        let item = SseItem::Ephemeral(Box::new(make_notification()));
        let sse = handle_sse_message(item).expect("should succeed");
        // SseMessage created without .id() — Debug output must not contain an id field.
        // Relies on Volga rendering id fields as `id: <value>\n` in its Debug output.
        // make_notification() produces {"jsonrpc":"2.0","method":"test"} — no "id:" in payload.
        let debug = format!("{:?}", sse);
        assert!(
            !debug.contains("id:"),
            "Ephemeral SSE must not have an id field"
        );
    }
}

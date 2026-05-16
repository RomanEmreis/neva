//! Engine-agnostic protocol handlers.
//!
//! These free functions contain all the JSON-RPC and MCP transport logic
//! that used to live inside Volga-shaped route handlers. They take a
//! neutral [`HttpRequest`] and an [`HttpContext`], and return a neutral
//! [`HttpResponse`] (or an [`SseResponse`] for the GET handler).

use crate::{
    auth::Claims,
    error::{Error, ErrorCode},
    types::{Message, RequestId, Response},
};
use bytes::Bytes;
use futures_util::{Stream, StreamExt, future::Either, stream};
use http::{HeaderMap, HeaderValue};
use std::pin::Pin;
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;

use super::{
    context::HttpContext,
    engine::HttpEngine,
    types::{HttpRequest, HttpResponse, SseResponse},
};

pub(crate) const MCP_SESSION_ID: &str = "Mcp-Session-Id";

/// One-call POST pipeline for engine adapters: convert the engine-native
/// request into neva's neutral form via [`HttpEngine::into_neutral`],
/// run the JSON-RPC dispatch via [`handle_post`], then convert the
/// neutral response back via [`HttpEngine::into_engine`].
///
/// Lets a route handler collapse to a single line, e.g. (axum):
///
/// ```rust,ignore
/// async fn post_handler(
///     State(ctx): State<Arc<HttpContext>>,
///     req: axum::Request<Body>,
/// ) -> axum::Response {
///     handlers::dispatch_post::<MyEngine>(req, &ctx).await
/// }
/// ```
///
/// **Authorization:** if the engine wants neva's per-tool / per-prompt /
/// per-resource role & permission gates to engage, it must insert an
/// `Arc<dyn neva::auth::Claims>` into `req.extensions_mut()` before
/// `into_neutral` returns (typically inside `HttpEngine::into_neutral`
/// or in the engine's route handler just before this call). See the
/// [`HttpEngine`] doc comment for the full contract.
pub async fn dispatch_post<E: HttpEngine>(req: E::Request, ctx: &HttpContext) -> E::Response {
    let neutral = E::into_neutral(req).await;
    let resp = handle_post(neutral, ctx).await;
    E::into_engine(resp)
}

/// One-call DELETE pipeline for engine adapters. See [`dispatch_post`].
pub async fn dispatch_delete<E: HttpEngine>(req: E::Request, ctx: &HttpContext) -> E::Response {
    let neutral = E::into_neutral(req).await;
    let resp = handle_delete(neutral, ctx).await;
    E::into_engine(resp)
}

/// One-call GET-SSE pipeline for engine adapters: converts the
/// engine-native request to neutral and runs the GET-SSE handshake.
///
/// The returned [`SseResponse`] is engine-agnostic; the engine still
/// matches `Stream { headers, stream }` (wrapping the stream in its
/// native SSE response type) vs `Status(resp)` (passing `resp` through
/// [`HttpEngine::into_engine`]).
pub async fn dispatch_get_sse<E: HttpEngine>(
    req: E::Request,
    ctx: &HttpContext,
) -> SseResponse<impl Stream<Item = E::SseEvent> + Send + 'static> {
    let neutral = E::into_neutral(req).await;
    handle_get_sse::<E>(neutral, ctx).await
}

/// Handle a POST `/{endpoint}` request — the JSON-RPC message ingress.
///
/// All MCP protocol logic lives here: parse body, classify as
/// request/notification/batch, run the init pre-register, attach claims
/// from `req.extensions()`, push the message onto the inbound channel,
/// and await the response on a oneshot (for requests) or return 202
/// immediately (for notifications and notification-only batches).
///
/// # Example
///
/// ```rust,ignore
/// let resp = handle_post(req, &ctx).await;
/// // engine translates `resp` into its native response type
/// ```
pub async fn handle_post(req: HttpRequest, ctx: &HttpContext) -> HttpResponse {
    let mut headers = req.headers().clone();
    let id = get_or_create_mcp_session(&headers);
    // Engine-neutral claims pickup: any engine that decoded auth claims
    // for this request is expected to insert them as
    // `Arc<dyn neva::auth::Claims>` into `req.extensions_mut()` before
    // calling `dispatch_post`. Per-tool/prompt/resource role and
    // permission gates then run against whatever concrete claims type
    // the engine supplied.
    let claims = req.extensions().get::<Arc<dyn Claims>>().cloned();
    let body = req.into_body();

    let msg = match parse_message(&body) {
        Ok(msg) => msg,
        Err(code) => {
            let resp = Response::error(RequestId::Null, Error::from(code));
            return build_json_response(http::StatusCode::OK, id, &Message::Response(resp));
        }
    };

    // Pre-register on the initialize handshake so the server can emit
    // events between the init POST response and the SSE GET.
    if let Message::Request(ref r) = msg
        && r.method == crate::commands::INIT
    {
        ctx.sse_registry.pre_register(id);
    }

    // Notification fast-path: 202 Accepted, no oneshot.
    if matches!(msg, Message::Notification(_)) {
        let msg = msg.set_session_id(id);
        let _ = ctx.inbound_tx.send(Ok(msg)).await;
        return status_response(http::StatusCode::ACCEPTED, id);
    }

    // Batch-of-notifications fast-path.
    if let Message::Batch(ref batch) = msg
        && !batch.has_requests()
        && !batch.has_error_responses()
    {
        let msg = msg.set_session_id(id);
        if ctx.inbound_tx.send(Ok(msg)).await.is_err() {
            return status_response(http::StatusCode::INTERNAL_SERVER_ERROR, id);
        }
        return status_response(http::StatusCode::ACCEPTED, id);
    }

    // Strip Authorization before forwarding (claims are already extracted).
    headers.remove(http::header::AUTHORIZATION);

    let mut msg = msg.set_session_id(id).set_headers(headers);
    if let Some(c) = claims {
        msg = msg.set_claims(c);
    }

    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel::<Message>();
    // full_id() takes &self, so we can compute the key before moving msg
    // into the send. RequestId is not Clone — the original handler used
    // the same insert-then-send order. The pending entry is reaped by
    // the SSE registry's cleanup loop if the inbound send fails (rare —
    // the channel is sized for hundreds of in-flight requests).
    ctx.pending.insert(msg.full_id(), resp_tx);
    if ctx.inbound_tx.send(Ok(msg)).await.is_err() {
        return status_response(http::StatusCode::INTERNAL_SERVER_ERROR, id);
    }
    match resp_rx.await {
        Ok(resp) => build_json_response(http::StatusCode::OK, id, &resp),
        Err(_) => status_response(http::StatusCode::INTERNAL_SERVER_ERROR, id),
    }
}

/// Parse the body into a `Message`. Two-step decode per JSON-RPC 2.0 §5.1.
fn parse_message(body: &Bytes) -> Result<Message, ErrorCode> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| ErrorCode::ParseError)?;
    serde_json::from_value::<Message>(value).map_err(|_| ErrorCode::InvalidRequest)
}

fn get_or_create_mcp_session(headers: &HeaderMap) -> uuid::Uuid {
    headers
        .get(MCP_SESSION_ID)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .unwrap_or_else(uuid::Uuid::new_v4)
}

fn build_json_response(
    status: http::StatusCode,
    session: uuid::Uuid,
    body: &Message,
) -> HttpResponse {
    let json = serde_json::to_vec(body).unwrap_or_default();
    let mut resp = http::Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Bytes::from(json))
        .unwrap_or_default();
    if let Ok(v) = HeaderValue::from_str(&session.to_string()) {
        resp.headers_mut().insert(MCP_SESSION_ID, v);
    }
    resp
}

fn status_response(status: http::StatusCode, session: uuid::Uuid) -> HttpResponse {
    let mut resp = http::Response::builder()
        .status(status)
        .body(Bytes::new())
        .unwrap_or_default();
    if let Ok(v) = HeaderValue::from_str(&session.to_string()) {
        resp.headers_mut().insert(MCP_SESSION_ID, v);
    }
    resp
}

/// Handle a DELETE `/{endpoint}` request — explicit session termination.
///
/// Returns 400 if `Mcp-Session-Id` is missing; otherwise terminates the
/// SSE session in the registry (and unregisters its log channel, when
/// tracing is enabled) and replies 200 with the session id echoed back.
pub async fn handle_delete(req: HttpRequest, ctx: &HttpContext) -> HttpResponse {
    let Some(id) = parse_session_id(req.headers()) else {
        return http::Response::builder()
            .status(http::StatusCode::BAD_REQUEST)
            .body(Bytes::new())
            .unwrap_or_default();
    };

    #[cfg(feature = "tracing")]
    crate::types::notification::fmt::LOG_REGISTRY.unregister(&id);
    ctx.sse_registry.terminate(&id);

    status_response(http::StatusCode::OK, id)
}

fn parse_session_id(headers: &HeaderMap) -> Option<uuid::Uuid> {
    headers
        .get(MCP_SESSION_ID)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
}

/// Internal item type used inside the GET handler — the engine's
/// `sse_tracked` / `sse_ephemeral` is invoked exactly once per emitted
/// event to produce the engine-native representation.
enum SseItem {
    Tracked(u64, Arc<Message>),
    Ephemeral(Box<Message>),
}

struct SseConnectionCleanup {
    id: uuid::Uuid,
    generation: u64,
    registry: Arc<crate::shared::SseSessionRegistry>,
}

impl Drop for SseConnectionCleanup {
    fn drop(&mut self) {
        #[cfg(feature = "tracing")]
        crate::types::notification::fmt::LOG_REGISTRY
            .unregister_if_generation(&self.id, self.generation);
        self.registry.unregister(&self.id, self.generation);
    }
}

/// Handle a GET `/{endpoint}` request — SSE stream subscribe.
///
/// Returns `SseResponse::Status(400)` if the session id is missing,
/// otherwise opens (or reconnects to) the session in the SSE registry
/// and returns `SseResponse::Stream { headers, stream }` where `stream`
/// is an `impl Stream<Item = E::SseEvent>` produced by calling the
/// engine's [`HttpEngine::sse_tracked`] / [`HttpEngine::sse_ephemeral`]
/// for each underlying `SseItem`.
///
/// The stream takes ownership of an `SseConnectionCleanup` drop-guard
/// that unregisters the session from the registry (and the log
/// registry, when tracing is on) when the connection closes.
pub async fn handle_get_sse<E: HttpEngine>(
    req: HttpRequest,
    ctx: &HttpContext,
) -> SseResponse<impl Stream<Item = E::SseEvent> + Send + 'static> {
    let Some(id) = parse_session_id(req.headers()) else {
        return SseResponse::Status(
            http::Response::builder()
                .status(http::StatusCode::BAD_REQUEST)
                .body(Bytes::new())
                .unwrap_or_default(),
        );
    };

    let (msg_tx, msg_rx) =
        tokio::sync::mpsc::channel::<(u64, Arc<Message>)>(ctx.sse_live_queue_capacity);
    let (_log_tx, log_rx) = tokio::sync::mpsc::channel::<Message>(ctx.sse_log_queue_capacity);

    let generation = ctx.sse_registry.register(id, msg_tx);
    #[cfg(feature = "tracing")]
    crate::types::notification::fmt::LOG_REGISTRY.register(id, generation, _log_tx);

    let last_seq: Option<u64> = req
        .headers()
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok());

    let replay = match last_seq {
        Some(seq) => ctx.sse_registry.replay_since(&id, seq),
        None => ctx.sse_registry.replay_all(&id),
    };

    let msg_stream = if replay.is_empty() {
        Either::Left(ReceiverStream::new(msg_rx).map(|(seq, arc)| SseItem::Tracked(seq, arc)))
    } else {
        let replay_end_seq = replay.last().map(|(s, _)| *s).unwrap_or(0);
        let replay_stream = stream::iter(replay).map(|(seq, arc)| SseItem::Tracked(seq, arc));
        let live = ReceiverStream::new(msg_rx)
            .filter(move |&(seq, _)| {
                let keep = seq > replay_end_seq;
                async move { keep }
            })
            .map(|(seq, arc)| SseItem::Tracked(seq, arc));
        Either::Right(replay_stream.chain(live))
    };

    let log_stream = ReceiverStream::new(log_rx).map(|m| SseItem::Ephemeral(Box::new(m)));

    let merged = stream::select(log_stream, msg_stream);
    let cleanup = SseConnectionCleanup {
        id,
        generation,
        registry: ctx.sse_registry.clone(),
    };
    let mut merged = Box::pin(merged);
    let guarded = stream::poll_fn(move |cx| {
        let _cleanup = &cleanup;
        Pin::new(&mut merged).poll_next(cx)
    })
    .map(|item| match item {
        SseItem::Tracked(seq, msg) => E::sse_tracked(seq, &msg),
        SseItem::Ephemeral(msg) => E::sse_ephemeral(&msg),
    });

    let mut headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(&id.to_string()) {
        headers.insert(MCP_SESSION_ID, v);
    }

    SseResponse::Stream {
        headers,
        stream: guarded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::SseSessionRegistry;
    use bytes::Bytes;
    use dashmap::DashMap;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    fn make_ctx() -> (
        HttpContext,
        mpsc::Receiver<Result<crate::types::Message, crate::error::Error>>,
    ) {
        let (inbound_tx, inbound_rx) =
            mpsc::channel::<Result<crate::types::Message, crate::error::Error>>(8);
        let ctx = HttpContext {
            addr: "127.0.0.1:0".into(),
            endpoint: "/mcp".into(),
            pending: Arc::new(DashMap::new()),
            sse_registry: Arc::new(SseSessionRegistry::new(8)),
            inbound_tx,
            sse_live_queue_capacity: 64,
            sse_log_queue_capacity: 64,
        };
        (ctx, inbound_rx)
    }

    fn make_request_body(method: &str) -> Bytes {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "id": 1
        });
        Bytes::from(serde_json::to_vec(&body).unwrap())
    }

    fn make_notification_body(method: &str) -> Bytes {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method
        });
        Bytes::from(serde_json::to_vec(&body).unwrap())
    }

    #[tokio::test]
    async fn notification_returns_202_without_pending_entry() {
        let (ctx, mut _rx) = make_ctx();
        let req = http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .body(make_notification_body("notifications/cancelled"))
            .unwrap();
        let resp = handle_post(req, &ctx).await;
        assert_eq!(resp.status(), http::StatusCode::ACCEPTED);
        assert!(
            ctx.pending.is_empty(),
            "no pending oneshot for notifications"
        );
    }

    #[tokio::test]
    async fn malformed_json_returns_parse_error_response() {
        let (ctx, _rx) = make_ctx();
        let req = http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .body(Bytes::from_static(b"not json"))
            .unwrap();
        let resp = handle_post(req, &ctx).await;
        assert_eq!(resp.status(), http::StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(body["error"]["code"], -32700);
    }

    #[tokio::test]
    async fn invalid_message_shape_returns_invalid_request() {
        let (ctx, _rx) = make_ctx();
        let req = http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .body(Bytes::from_static(b"{\"valid_json\": true}"))
            .unwrap();
        let resp = handle_post(req, &ctx).await;
        assert_eq!(resp.status(), http::StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(body["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn init_request_pre_registers_session() {
        let (ctx, _rx) = make_ctx();
        let req = http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .body(make_request_body(crate::commands::INIT))
            .unwrap();
        let ctx_arc = std::sync::Arc::new(ctx);
        let ctx_clone = ctx_arc.clone();
        let _h = tokio::spawn(async move {
            handle_post(req, &ctx_clone).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // After pre_register, the registry has at least one tracked session.
        // We can't easily inspect it via public API; assert that pending has
        // exactly one entry (the oneshot for the init request).
        assert_eq!(ctx_arc.pending.len(), 1);
    }

    #[tokio::test]
    async fn delete_without_session_id_returns_400() {
        let (ctx, _rx) = make_ctx();
        let req = http::Request::builder()
            .method("DELETE")
            .uri("/mcp")
            .body(Bytes::new())
            .unwrap();
        let resp = handle_delete(req, &ctx).await;
        assert_eq!(resp.status(), http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delete_with_session_id_echoes_it_back() {
        let (ctx, _rx) = make_ctx();
        let id = uuid::Uuid::new_v4();
        let req = http::Request::builder()
            .method("DELETE")
            .uri("/mcp")
            .header(MCP_SESSION_ID, id.to_string())
            .body(Bytes::new())
            .unwrap();
        let resp = handle_delete(req, &ctx).await;
        assert_eq!(resp.status(), http::StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get(MCP_SESSION_ID)
                .and_then(|v| v.to_str().ok()),
            Some(id.to_string().as_str())
        );
    }

    /// Minimal `HttpEngine` impl used only to exercise `handle_get_sse`
    /// in unit tests. `into_neutral` / `into_engine` / `run` are not
    /// invoked by these tests so they are left as `unreachable!()`.
    struct TestEngine;

    impl super::HttpEngine for TestEngine {
        type Request = HttpRequest;
        type Response = HttpResponse;
        type SseEvent = (Option<u64>, String);

        async fn into_neutral(_req: Self::Request) -> HttpRequest {
            unreachable!()
        }
        fn into_engine(_resp: HttpResponse) -> Self::Response {
            unreachable!()
        }
        fn sse_tracked(seq: u64, msg: &Message) -> Self::SseEvent {
            (Some(seq), serde_json::to_string(msg).unwrap())
        }
        fn sse_ephemeral(msg: &Message) -> Self::SseEvent {
            (None, serde_json::to_string(msg).unwrap())
        }
        async fn run(
            self,
            _ctx: HttpContext,
            _token: tokio_util::sync::CancellationToken,
        ) -> Result<(), crate::error::Error> {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn get_without_session_id_returns_400() {
        let (ctx, _rx) = make_ctx();
        let req = http::Request::builder()
            .method("GET")
            .uri("/mcp")
            .body(Bytes::new())
            .unwrap();
        let resp = handle_get_sse::<TestEngine>(req, &ctx).await;
        match resp {
            SseResponse::Status(r) => assert_eq!(r.status(), http::StatusCode::BAD_REQUEST),
            SseResponse::Stream { .. } => panic!("expected Status, got Stream"),
        }
    }

    #[tokio::test]
    async fn get_with_session_returns_stream_with_session_header() {
        let (ctx, _rx) = make_ctx();
        let id = uuid::Uuid::new_v4();
        ctx.sse_registry.pre_register(id);
        let req = http::Request::builder()
            .method("GET")
            .uri("/mcp")
            .header(MCP_SESSION_ID, id.to_string())
            .body(Bytes::new())
            .unwrap();
        let resp = handle_get_sse::<TestEngine>(req, &ctx).await;
        match resp {
            SseResponse::Stream { headers, stream: _ } => {
                assert_eq!(
                    headers.get(MCP_SESSION_ID).and_then(|v| v.to_str().ok()),
                    Some(id.to_string().as_str())
                );
            }
            SseResponse::Status(_) => panic!("expected Stream, got Status"),
        }
    }
}

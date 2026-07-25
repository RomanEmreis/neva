//! Engine-agnostic protocol handlers.
//!
//! These free functions contain all the JSON-RPC and MCP transport logic
//! that used to live inside Volga-shaped route handlers. They take a
//! neutral [`HttpRequest`] and an [`HttpContext`], and return a neutral
//! [`StreamResponse`] (POST and GET) or [`HttpResponse`] (DELETE, metadata).

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
    types::{HttpRequest, HttpResponse, StreamResponse},
};

pub(crate) const MCP_SESSION_ID: &str = "Mcp-Session-Id";

/// One-call POST pipeline for engine adapters: convert the engine-native
/// request into neva's neutral form via [`HttpEngine::adapt_request`] and
/// run the JSON-RPC dispatch.
///
/// The result is a [`StreamResponse`] -- the two reply shapes Streamable
/// HTTP allows on POST (both since spec revision 2025-03-26):
///
/// * `Complete(resp)` -- a single-body reply (JSON object, batch array, or a
///   bare status such as `202`); pass it through
///   [`HttpEngine::adapt_response`].
/// * `Stream { stream, .. }` -- a request-scoped SSE stream carrying the
///   request's `notifications/message` / `notifications/progress` followed by
///   its final response; frame it exactly like the GET stream. Produced under
///   `proto-2026-07-28-rc` + `tracing` when the request opts in (carries
///   `io.modelcontextprotocol/logLevel` or a `progressToken` in `_meta`);
///   other builds always return `Complete`.
///
/// Returns [`Err`] when [`HttpEngine::adapt_request`] fails. Engines whose
/// native response type is itself a `Result` can integrate with `?`;
/// engines whose response type is infallible can map the error onto an
/// HTTP 500 of their choosing.
///
/// A route handler is the same two-arm match the GET route already has,
/// e.g. (axum):
///
/// ```rust,ignore
/// async fn post_handler(
///     State(ctx): State<HttpContext>,
///     req: axum::Request<Body>,
/// ) -> Result<axum::Response, MyError> {
///     match handlers::dispatch_post::<MyEngine>(req, &ctx).await? {
///         StreamResponse::Stream { stream, .. } => sse_response(stream),
///         StreamResponse::Complete(resp) => Ok(MyEngine::adapt_response(resp)),
///     }
/// }
/// ```
///
/// **Authorization:** if the engine wants neva's per-tool / per-prompt /
/// per-resource role & permission gates to engage, it must insert an
/// `Arc<dyn neva::auth::Claims>` into `req.extensions_mut()` before
/// `adapt_request` returns (typically inside `HttpEngine::adapt_request`
/// or in the engine's route handler just before this call). See the
/// [`HttpEngine`] doc comment for the full contract.
pub async fn dispatch_post<E: HttpEngine>(
    req: E::Request,
    ctx: &HttpContext,
) -> Result<StreamResponse<impl Stream<Item = E::SseEvent> + Send + 'static>, Error> {
    let neutral = E::adapt_request(req).await?;
    #[cfg(all(feature = "proto-2026-07-28-rc", feature = "tracing"))]
    {
        Ok(handle_post_streaming::<E>(neutral, ctx).await)
    }
    // Without the RC flag (or without tracing to source the notifications)
    // every POST reply is a single body; the Stream arm is never produced.
    #[cfg(not(all(feature = "proto-2026-07-28-rc", feature = "tracing")))]
    {
        let resp = handle_post(neutral, ctx).await;
        Ok(StreamResponse::<stream::Empty<E::SseEvent>>::Complete(resp))
    }
}

/// One-call DELETE pipeline for engine adapters. See [`dispatch_post`].
pub async fn dispatch_delete<E: HttpEngine>(
    req: E::Request,
    ctx: &HttpContext,
) -> Result<E::Response, Error> {
    let neutral = E::adapt_request(req).await?;
    let resp = handle_delete(neutral, ctx).await;
    Ok(E::adapt_response(resp))
}

/// One-call GET-SSE pipeline for engine adapters: converts the
/// engine-native request to neutral and runs the GET-SSE handshake.
///
/// The returned [`StreamResponse`] is engine-agnostic; the engine still
/// matches `Stream { headers, stream }` (wrapping the stream in its
/// native SSE response type) vs `Complete(resp)` (passing `resp` through
/// [`HttpEngine::adapt_response`]).
///
/// Returns [`Err`] when [`HttpEngine::adapt_request`] fails -- same
/// rationale as [`dispatch_post`].
pub async fn dispatch_get_sse<E: HttpEngine>(
    req: E::Request,
    ctx: &HttpContext,
) -> Result<StreamResponse<impl Stream<Item = E::SseEvent> + Send + 'static>, Error> {
    let neutral = E::adapt_request(req).await?;
    Ok(handle_get_sse::<E>(neutral, ctx).await)
}

/// Handle a POST `/{endpoint}` request -- the JSON-RPC message ingress,
/// always replying with a single body (JSON object, batch array, or status).
///
/// This is the JSON-only building block: parse body, classify as
/// request/notification/batch, run the init pre-register, attach claims
/// from `req.extensions()`, push the message onto the inbound channel,
/// and await the response on a oneshot (for requests) or return 202
/// immediately (for notifications and notification-only batches).
///
/// Prefer [`dispatch_post`], which also covers the request-scoped SSE reply
/// (`StreamResponse::Stream`) the RC transport produces for requests that opt
/// into notifications; use this directly only when the engine cannot stream.
///
/// # Example
///
/// ```rust,ignore
/// let resp = handle_post(req, &ctx).await;
/// // engine translates `resp` into its native response type
/// ```
pub async fn handle_post(req: HttpRequest, ctx: &HttpContext) -> HttpResponse {
    match prepare_post(req, ctx).await {
        PostPrep::Reply(resp) => resp,
        PostPrep::Dispatch { id, msg } => {
            let (resp_tx, resp_rx) = tokio::sync::oneshot::channel::<Message>();
            ctx.pending.insert(msg.full_id(), resp_tx);
            if ctx.inbound_tx.send(Ok(msg)).await.is_err() {
                return status_response(http::StatusCode::INTERNAL_SERVER_ERROR, id);
            }
            match resp_rx.await {
                Ok(resp) => build_json_response(http::StatusCode::OK, id, &resp),
                Err(_) => status_response(http::StatusCode::INTERNAL_SERVER_ERROR, id),
            }
        }
    }
}

/// Outcome of the shared POST preamble: either an early reply (protocol error,
/// parse error, or a `202` for a notification/notification-only batch, all with
/// side effects already applied), or a request ready to dispatch.
enum PostPrep {
    /// A fully-formed reply -- return it as-is.
    Reply(HttpResponse),
    /// A request to dispatch: `msg` already carries its session id, headers, and
    /// claims; `id` is the per-`POST` session id used for response framing.
    Dispatch { id: uuid::Uuid, msg: Message },
}

/// Runs the transport preamble shared by the JSON and streaming POST paths:
/// protocol-version validation, body parse, trace-context recording, and the
/// notification fast-paths (which forward to the runtime and reply `202`).
async fn prepare_post(req: HttpRequest, ctx: &HttpContext) -> PostPrep {
    let mut headers = req.headers().clone();
    let id = get_or_create_mcp_session(&headers);

    // Stateless RC transport requires every POST to carry the exact RC
    // `MCP-Protocol-Version` header; reject before body dispatch otherwise.
    // `PROTOCOL_VERSIONS` still lists legacy versions (e.g. 2025-06-18) for
    // the non-RC build, but this build has removed the legacy initialize/SSE
    // behavior and only speaks RC stateless semantics -- so a client/proxy
    // advertising a legacy version must be rejected, not silently served
    // under RC. Compare against the fixed RC version (the last/only RC entry)
    // rather than the whole compatibility list.
    #[cfg(feature = "proto-2026-07-28-rc")]
    {
        let rc_version = crate::PROTOCOL_VERSIONS.last().copied();
        let ok = headers
            .get(crate::transport::http::MCP_PROTOCOL_VERSION)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| Some(v) == rc_version);

        if !ok {
            let resp = Response::error(
                RequestId::Null,
                Error::new(
                    ErrorCode::InvalidRequest,
                    "Missing or unsupported MCP-Protocol-Version header",
                ),
            );
            return PostPrep::Reply(build_json_response(
                http::StatusCode::OK,
                id,
                &Message::Response(resp),
            ));
        }
    }
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
            return PostPrep::Reply(build_json_response(
                http::StatusCode::OK,
                id,
                &Message::Response(resp),
            ));
        }
    };

    // Passive W3C Trace Context recorder: when both `proto-2026-07-28-rc`
    // and `tracing` are enabled, record any `_meta.traceparent` /
    // `_meta.tracestate` on the active span. `Span::current().record(...)`
    // is a no-op unless the caller's span declares these fields via
    // `#[instrument(fields(traceparent, tracestate))]`.
    #[cfg(all(feature = "proto-2026-07-28-rc", feature = "tracing"))]
    if let Message::Request(ref r) = msg
        && let Some(meta) = r
            .params
            .as_ref()
            .and_then(|p| p.get("_meta"))
            .and_then(|m| m.as_object())
    {
        if let Some(tp) = meta.get("traceparent").and_then(|v| v.as_str()) {
            tracing::Span::current().record("traceparent", tp);
        }
        if let Some(ts) = meta.get("tracestate").and_then(|v| v.as_str()) {
            tracing::Span::current().record("tracestate", ts);
        }
    }

    // Pre-register on the initialize handshake so the server can emit
    // events between the init POST response and the SSE GET. Stateless RC
    // transport has no SSE GET, so this is skipped under the flag.
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
    if let Message::Request(ref r) = msg
        && r.method == crate::commands::INIT
    {
        ctx.sse_registry.pre_register(id);
    }

    // Notification fast-path: 202 Accepted, no oneshot.
    if matches!(msg, Message::Notification(_)) {
        let msg = msg.set_session_id(id);
        let _ = ctx.inbound_tx.send(Ok(msg)).await;
        return PostPrep::Reply(status_response(http::StatusCode::ACCEPTED, id));
    }

    // Batch-of-notifications fast-path.
    if let Message::Batch(ref batch) = msg
        && !batch.has_requests()
        && !batch.has_error_responses()
    {
        let msg = msg.set_session_id(id);
        if ctx.inbound_tx.send(Ok(msg)).await.is_err() {
            return PostPrep::Reply(status_response(http::StatusCode::INTERNAL_SERVER_ERROR, id));
        }
        return PostPrep::Reply(status_response(http::StatusCode::ACCEPTED, id));
    }

    // Strip Authorization before forwarding (claims are already extracted).
    headers.remove(http::header::AUTHORIZATION);

    let mut msg = msg.set_session_id(id).set_headers(headers);
    if let Some(c) = claims {
        msg = msg.set_claims(c);
    }

    PostPrep::Dispatch { id, msg }
}

/// The RC arm of [`dispatch_post`]: a POST pipeline that can return a
/// request-scoped SSE response, mirroring [`handle_get_sse`].
///
/// When the request opts into request-scoped notifications (carries
/// `io.modelcontextprotocol/logLevel` or a `progressToken` in `_meta`), the
/// reply is an SSE stream: notifications produced while handling the request
/// flow first (routed via the per-request sink), then the final response closes
/// the stream. Otherwise the reply is a single JSON object (`Complete`),
/// exactly as [`handle_post`].
#[cfg(all(feature = "proto-2026-07-28-rc", feature = "tracing"))]
async fn handle_post_streaming<E: HttpEngine>(
    req: HttpRequest,
    ctx: &HttpContext,
) -> StreamResponse<impl Stream<Item = E::SseEvent> + Send + 'static> {
    match prepare_post(req, ctx).await {
        PostPrep::Reply(resp) => StreamResponse::Complete(resp),
        PostPrep::Dispatch { id, msg } => {
            let (resp_tx, resp_rx) = tokio::sync::oneshot::channel::<Message>();
            let full_id = msg.full_id();
            ctx.pending.insert(full_id.clone(), resp_tx);

            if !opts_into_notifications(&msg) {
                if ctx.inbound_tx.send(Ok(msg)).await.is_err() {
                    ctx.pending.remove(&full_id);
                    return StreamResponse::Complete(status_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        id,
                    ));
                }
                return match resp_rx.await {
                    Ok(resp) => StreamResponse::Complete(build_json_response(
                        http::StatusCode::OK,
                        id,
                        &resp,
                    )),
                    Err(_) => StreamResponse::Complete(status_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        id,
                    )),
                };
            }

            // Opted in: register the per-request notification sink (keyed by the
            // per-POST session id, which the tracing span carries) before the
            // runtime starts handling, then stream notifications + response.
            let notif_rx = crate::types::notification::fmt::register_request_sink(
                id,
                ctx.sse_log_queue_capacity,
            );

            if ctx.inbound_tx.send(Ok(msg)).await.is_err() {
                crate::types::notification::fmt::unregister_request_sink(&id);
                ctx.pending.remove(&full_id);
                return StreamResponse::Complete(status_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    id,
                ));
            }

            let stream =
                post_notification_stream(id, full_id, ctx.pending.clone(), notif_rx, resp_rx)
                    .map(|msg| E::ephemeral_event(&msg));

            StreamResponse::Stream {
                headers: HeaderMap::new(),
                stream,
            }
        }
    }
}

/// Whether a message opts into request-scoped notifications: a request -- or,
/// for a batch, *any* contained request -- carrying `logLevel` or
/// `progressToken` in `_meta`.
///
/// Batches count because a client (e.g. via `Client::apply_client_meta_to_batch`)
/// stamps the configured level onto every batched request; the inner requests
/// share this POST's session id (copied in `execute_batch`), so their
/// notifications route to the one sink and stream on this single response.
#[cfg(all(feature = "proto-2026-07-28-rc", feature = "tracing"))]
fn opts_into_notifications(msg: &Message) -> bool {
    match msg {
        Message::Request(r) => request_opts_in(r),
        Message::Batch(batch) => batch.iter().any(
            |env| matches!(env, crate::types::MessageEnvelope::Request(r) if request_opts_in(r)),
        ),
        _ => false,
    }
}

/// Whether a single request carries `logLevel` or `progressToken` in `_meta`.
#[cfg(all(feature = "proto-2026-07-28-rc", feature = "tracing"))]
fn request_opts_in(req: &crate::types::Request) -> bool {
    req.params
        .as_ref()
        .and_then(|p| p.get("_meta"))
        .and_then(|m| m.as_object())
        .is_some_and(|meta| {
            meta.contains_key("io.modelcontextprotocol/logLevel")
                || meta.contains_key("progressToken")
        })
}

/// Builds the request-scoped SSE body: notifications flow as they arrive
/// (biased ahead of the response), then the final response closes the stream.
///
/// The response is *buffered* rather than emitted on arrival: the terminal
/// `message_middleware` completes it while user middleware wrapped around
/// `next(ctx)` may still be running and logging. The stream therefore stays open
/// until the notification channel closes -- which happens when the whole
/// pipeline is done and `App`'s sink guard drops the sender (see
/// `RequestSinkGuard`) -- drains what is queued, and emits the response last.
///
/// Dropping the stream (end of body or client disconnect) unregisters the
/// per-request sink and clears the pending entry.
#[cfg(all(feature = "proto-2026-07-28-rc", feature = "tracing"))]
fn post_notification_stream(
    id: uuid::Uuid,
    full_id: RequestId,
    pending: super::context::RequestMap,
    notif_rx: tokio::sync::mpsc::Receiver<Message>,
    resp_rx: tokio::sync::oneshot::Receiver<Message>,
) -> impl Stream<Item = Message> + Send {
    struct Cleanup {
        id: uuid::Uuid,
        full_id: RequestId,
        pending: super::context::RequestMap,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            crate::types::notification::fmt::unregister_request_sink(&self.id);
            self.pending.remove(&self.full_id);
        }
    }

    struct State {
        notif_rx: tokio::sync::mpsc::Receiver<Message>,
        /// Taken once the response arrives (or the channel is known dead).
        resp_rx: Option<tokio::sync::oneshot::Receiver<Message>>,
        /// The buffered final response, emitted after the last notification.
        response: Option<Message>,
        /// Whether more notifications may still arrive.
        notifs_open: bool,
        /// Holds the cleanup guard until the stream is fully consumed.
        _cleanup: Cleanup,
    }

    /// What one poll of the two channels produced.
    enum Step {
        Notification(Message),
        NotificationsClosed,
        Response(Option<Message>),
    }

    let state = State {
        notif_rx,
        resp_rx: Some(resp_rx),
        response: None,
        notifs_open: true,
        _cleanup: Cleanup {
            id,
            full_id,
            pending,
        },
    };

    stream::unfold(state, |mut state| async move {
        while state.notifs_open {
            // Split the borrows so both channels can be polled in one `select!`.
            let step = {
                let State {
                    notif_rx, resp_rx, ..
                } = &mut state;
                match resp_rx.as_mut() {
                    Some(rx) => tokio::select! {
                        biased;
                        n = notif_rx.recv() => match n {
                            Some(n) => Step::Notification(n),
                            None => Step::NotificationsClosed,
                        },
                        r = rx => Step::Response(r.ok()),
                    },
                    // Response already in hand: keep draining notifications.
                    None => match notif_rx.recv().await {
                        Some(n) => Step::Notification(n),
                        None => Step::NotificationsClosed,
                    },
                }
            };

            match step {
                Step::Notification(n) => return Some((n, state)),
                Step::NotificationsClosed => state.notifs_open = false,
                Step::Response(Some(resp)) => {
                    // Buffer it and keep draining until the pipeline is done.
                    state.response = Some(resp);
                    state.resp_rx = None;
                }
                // The response channel was dropped: the runtime will never
                // answer, so stop waiting on notifications and end the body
                // rather than holding the connection open.
                Step::Response(None) => {
                    state.resp_rx = None;
                    state.notifs_open = false;
                }
            }
        }

        // Pipeline finished and every notification is drained; close with the
        // response. A dropped response channel (runtime gone) just ends the body.
        if let Some(rx) = state.resp_rx.take() {
            state.response = rx.await.ok();
        }
        state.response.take().map(|resp| (resp, state))
    })
}

/// Parse the body into a [`Message`].
///
/// Single-step decode: `serde_json::Error::classify()` distinguishes
/// JSON-RPC 2.0 section 5.1 ParseError (`Category::Syntax` / `Category::Eof` --
/// the body is not valid JSON) from InvalidRequest (`Category::Data` --
/// the body is valid JSON but does not match any [`Message`] variant).
fn parse_message(body: &Bytes) -> Result<Message, ErrorCode> {
    serde_json::from_slice::<Message>(body).map_err(|e| match e.classify() {
        serde_json::error::Category::Syntax | serde_json::error::Category::Eof => {
            ErrorCode::ParseError
        }
        _ => ErrorCode::InvalidRequest,
    })
}

fn get_or_create_mcp_session(
    #[cfg_attr(feature = "proto-2026-07-28-rc", allow(unused_variables))] headers: &HeaderMap,
) -> uuid::Uuid {
    // RC removed protocol-level sessions and the `Mcp-Session-Id` header, and
    // this id doubles as the per-POST correlation key for the pending-response
    // slot and the request notification sink. Mint a fresh one per POST so a
    // client-supplied (or proxied) header can never collide two concurrent
    // stateless requests onto the same sink/slot.
    #[cfg(feature = "proto-2026-07-28-rc")]
    {
        uuid::Uuid::new_v4()
    }
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
    headers
        .get(MCP_SESSION_ID)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .unwrap_or_else(uuid::Uuid::new_v4)
}

fn build_json_response(
    status: http::StatusCode,
    #[cfg_attr(feature = "proto-2026-07-28-rc", allow(unused_variables))] session: uuid::Uuid,
    body: &Message,
) -> HttpResponse {
    let json = serde_json::to_vec(body).unwrap_or_default();
    #[cfg_attr(feature = "proto-2026-07-28-rc", allow(unused_mut))]
    let mut resp = http::Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Bytes::from(json))
        .unwrap_or_default();
    // Stateless RC transport never puts the session id on the wire.
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
    if let Ok(v) = HeaderValue::from_str(&session.to_string()) {
        resp.headers_mut().insert(MCP_SESSION_ID, v);
    }
    resp
}

fn status_response(
    status: http::StatusCode,
    #[cfg_attr(feature = "proto-2026-07-28-rc", allow(unused_variables))] session: uuid::Uuid,
) -> HttpResponse {
    #[cfg_attr(feature = "proto-2026-07-28-rc", allow(unused_mut))]
    let mut resp = http::Response::builder()
        .status(status)
        .body(Bytes::new())
        .unwrap_or_default();
    // Stateless RC transport never puts the session id on the wire.
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
    if let Ok(v) = HeaderValue::from_str(&session.to_string()) {
        resp.headers_mut().insert(MCP_SESSION_ID, v);
    }
    resp
}

/// Handle a DELETE `/{endpoint}` request -- explicit session termination.
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

/// Handle a GET on the well-known path -- serves the RFC 9728 Protected
/// Resource Metadata document pre-built at server start.
///
/// The engine mounts this on [`HttpContext::oauth_metadata_path`]; if the
/// route is reachable while OAuth is not configured, it answers 404.
///
/// # Example
///
/// ```rust,ignore
/// // in the engine's well-known route:
/// let resp = E::adapt_response(handlers::handle_oauth_metadata(&ctx));
/// ```
#[cfg(feature = "server-oauth")]
pub fn handle_oauth_metadata(ctx: &HttpContext) -> HttpResponse {
    let Some(oauth) = &ctx.oauth else {
        return http::Response::builder()
            .status(http::StatusCode::NOT_FOUND)
            .body(Bytes::new())
            .unwrap_or_default();
    };
    http::Response::builder()
        .status(http::StatusCode::OK)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(oauth.body.clone())
        .unwrap_or_default()
}

/// Build the 401 reply for a request that failed (or skipped) token
/// validation: `WWW-Authenticate: Bearer` with the `resource_metadata`
/// parameter pointing at the RFC 9728 document, so a client can start
/// the OAuth discovery flow. Falls back to a bare `Bearer` challenge
/// when OAuth is not configured.
///
/// The default Volga adapter emits its own challenge through Volga's
/// bearer pipeline -- this helper is for custom engines that validate
/// tokens themselves.
///
/// # Example
///
/// ```rust,ignore
/// // in a custom engine, when the bearer token is missing/invalid:
/// let resp = E::adapt_response(handlers::handle_unauthorized(&ctx));
/// ```
#[cfg(feature = "server-oauth")]
pub fn handle_unauthorized(ctx: &HttpContext) -> HttpResponse {
    let challenge = ctx
        .oauth
        .as_ref()
        .map_or("Bearer", |oauth| &*oauth.challenge);
    http::Response::builder()
        .status(http::StatusCode::UNAUTHORIZED)
        .header(http::header::WWW_AUTHENTICATE, challenge)
        .body(Bytes::new())
        .unwrap_or_default()
}

/// Internal item type used inside the GET handler -- the engine's
/// `tracked_event` / `ephemeral_event` is invoked exactly once per emitted
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

/// Handle a GET `/{endpoint}` request -- SSE stream subscribe.
///
/// Returns `StreamResponse::Complete(400)` if the session id is missing,
/// otherwise opens (or reconnects to) the session in the SSE registry
/// and returns `StreamResponse::Stream { headers, stream }` where `stream`
/// is an `impl Stream<Item = E::SseEvent>` produced by calling the
/// engine's [`HttpEngine::tracked_event`] / [`HttpEngine::ephemeral_event`]
/// for each underlying `SseItem`.
///
/// The stream takes ownership of an `SseConnectionCleanup` drop-guard
/// that unregisters the session from the registry (and the log
/// registry, when tracing is on) when the connection closes.
pub async fn handle_get_sse<E: HttpEngine>(
    req: HttpRequest,
    ctx: &HttpContext,
) -> StreamResponse<impl Stream<Item = E::SseEvent> + Send + 'static> {
    let Some(id) = parse_session_id(req.headers()) else {
        return StreamResponse::Complete(
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
        SseItem::Tracked(seq, msg) => E::tracked_event(seq, &msg),
        SseItem::Ephemeral(msg) => E::ephemeral_event(&msg),
    });

    let mut headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(&id.to_string()) {
        headers.insert(MCP_SESSION_ID, v);
    }

    StreamResponse::Stream {
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
            #[cfg(feature = "server-oauth")]
            oauth: None,
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

    /// A POST request builder that, under the RC flag, carries the required
    /// `MCP-Protocol-Version` header so it passes the stateless gate.
    fn post_builder() -> http::request::Builder {
        let b = http::Request::builder().method("POST").uri("/mcp");
        #[cfg(feature = "proto-2026-07-28-rc")]
        let b = b.header(crate::transport::http::MCP_PROTOCOL_VERSION, "2026-07-28");
        b
    }

    #[cfg(feature = "proto-2026-07-28-rc")]
    #[tokio::test]
    async fn rejects_missing_protocol_version() {
        let (ctx, _rx) = make_ctx();
        let req = http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .body(make_request_body("ping"))
            .unwrap();
        let resp = handle_post(req, &ctx).await;
        assert_eq!(resp.status(), http::StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(body["error"]["code"], -32600);
    }

    #[cfg(feature = "proto-2026-07-28-rc")]
    #[tokio::test]
    async fn rejects_unsupported_protocol_version() {
        let (ctx, _rx) = make_ctx();
        let req = http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(crate::transport::http::MCP_PROTOCOL_VERSION, "1999-01-01")
            .body(make_request_body("ping"))
            .unwrap();
        let resp = handle_post(req, &ctx).await;
        assert_eq!(resp.status(), http::StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(body["error"]["code"], -32600);
    }

    #[cfg(feature = "proto-2026-07-28-rc")]
    #[tokio::test]
    async fn rejects_legacy_protocol_version() {
        // A legacy version that IS in `PROTOCOL_VERSIONS` but is not the RC
        // version: the old `.contains()` gate accepted it even though this build
        // only speaks RC stateless semantics. It must be rejected.
        let (ctx, _rx) = make_ctx();
        let req = http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(crate::transport::http::MCP_PROTOCOL_VERSION, "2025-06-18")
            .body(make_request_body("ping"))
            .unwrap();
        let resp = handle_post(req, &ctx).await;
        assert_eq!(resp.status(), http::StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(body["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn notification_returns_202_without_pending_entry() {
        let (ctx, mut _rx) = make_ctx();
        let req = post_builder()
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
        let req = post_builder()
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
        let req = post_builder()
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
        let req = post_builder()
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

    // Session-id echo is intentionally removed under the stateless RC transport.
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
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
    /// in unit tests. `adapt_request` / `adapt_response` / `run` are not
    /// invoked by these tests so they are left as `unreachable!()`.
    struct TestEngine;

    impl super::HttpEngine for TestEngine {
        type Request = HttpRequest;
        type Response = HttpResponse;
        type SseEvent = (Option<u64>, String);

        async fn adapt_request(_req: Self::Request) -> Result<HttpRequest, crate::error::Error> {
            unreachable!()
        }
        fn adapt_response(_resp: HttpResponse) -> Self::Response {
            unreachable!()
        }
        fn tracked_event(seq: u64, msg: &Message) -> Self::SseEvent {
            (Some(seq), serde_json::to_string(msg).unwrap())
        }
        fn ephemeral_event(msg: &Message) -> Self::SseEvent {
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
            StreamResponse::Complete(r) => assert_eq!(r.status(), http::StatusCode::BAD_REQUEST),
            StreamResponse::Stream { .. } => panic!("expected Status, got Stream"),
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
            StreamResponse::Stream { headers, stream: _ } => {
                assert_eq!(
                    headers.get(MCP_SESSION_ID).and_then(|v| v.to_str().ok()),
                    Some(id.to_string().as_str())
                );
            }
            StreamResponse::Complete(_) => panic!("expected Stream, got Status"),
        }
    }

    #[cfg(feature = "server-oauth")]
    fn make_oauth_ctx() -> HttpContext {
        use crate::transport::http::core::oauth::OAuthResourceOptions;

        let (mut ctx, _rx) = make_ctx();
        let oauth = OAuthResourceOptions::default()
            .with_authorization_servers(["https://auth.example.com"])
            .resolve("http://127.0.0.1:3000/mcp")
            .unwrap();
        ctx.oauth = Some(oauth);
        ctx
    }

    #[cfg(feature = "server-oauth")]
    #[test]
    fn oauth_metadata_serves_the_configured_document() {
        let ctx = make_oauth_ctx();

        let resp = handle_oauth_metadata(&ctx);

        assert_eq!(resp.status(), http::StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get(http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("application/json")
        );
        let doc: serde_json::Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(doc["resource"], "http://127.0.0.1:3000/mcp");
        assert_eq!(doc["authorization_servers"][0], "https://auth.example.com");
    }

    #[cfg(feature = "server-oauth")]
    #[test]
    fn oauth_metadata_without_config_returns_404() {
        let (ctx, _rx) = make_ctx();
        let resp = handle_oauth_metadata(&ctx);
        assert_eq!(resp.status(), http::StatusCode::NOT_FOUND);
    }

    #[cfg(feature = "server-oauth")]
    #[test]
    fn unauthorized_challenge_points_at_resource_metadata() {
        use crate::transport::http::core::oauth::BearerChallenge;

        let ctx = make_oauth_ctx();

        let resp = handle_unauthorized(&ctx);

        assert_eq!(resp.status(), http::StatusCode::UNAUTHORIZED);
        let header = resp
            .headers()
            .get(http::header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok())
            .unwrap();
        let challenge = BearerChallenge::parse(header).unwrap();
        assert_eq!(
            challenge.resource_metadata(),
            Some("http://127.0.0.1:3000/.well-known/oauth-protected-resource/mcp")
        );
    }

    #[cfg(feature = "server-oauth")]
    #[test]
    fn unauthorized_without_config_sends_bare_bearer_challenge() {
        let (ctx, _rx) = make_ctx();

        let resp = handle_unauthorized(&ctx);

        assert_eq!(resp.status(), http::StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers()
                .get(http::header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer")
        );
    }
}

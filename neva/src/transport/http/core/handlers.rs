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
///   MCP 2026-07-28 + `tracing` when the request opts in (carries
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
    #[cfg(not(feature = "legacy-spec"))]
    {
        Ok(handle_post_streaming::<E>(neutral, ctx).await)
    }
    // Under `legacy-spec` every POST reply is a single body; the Stream arm is
    // never produced.
    #[cfg(feature = "legacy-spec")]
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
/// (`StreamResponse::Stream`) the 2026-07-28 transport produces for requests that opt
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
                Ok(resp) => build_json_response(dispatched_status(&resp), id, &resp),
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

    // Stateless 2026-07-28 transport requires every POST to carry the exact 2026-07-28
    // `MCP-Protocol-Version` header; reject before body dispatch otherwise.
    // `PROTOCOL_VERSIONS` still lists legacy versions (e.g. 2025-06-18) for
    // the legacy build, but this build has removed the legacy initialize/SSE
    // behavior and only speaks 2026-07-28 stateless semantics -- so a client/proxy
    // advertising a legacy version must be rejected, not silently served
    // under MCP 2026-07-28. Compare against the fixed 2026-07-28 version (the last/only 2026-07-28 entry)
    // rather than the whole compatibility list.
    //
    // The verdict is reached here but delivered after the body is parsed: a
    // JSON-RPC error reaches the caller only if it carries the id the caller is
    // waiting on, and the id is in the body. Nothing between the two points
    // depends on the version being right.
    #[cfg(not(feature = "legacy-spec"))]
    let version_err;
    #[cfg(not(feature = "legacy-spec"))]
    {
        let header = headers
            .get(crate::transport::http::MCP_PROTOCOL_VERSION)
            .and_then(|v| v.to_str().ok());

        // A missing or unreadable header is a header problem (-32020); a
        // well-formed header naming a version this build does not speak is a
        // version problem (-32022), and the client is told what is on offer so
        // it can retry. Both answer `400 Bad Request` per the spec.
        version_err = match header {
            None => Some(Error::new(
                ErrorCode::HeaderMismatch,
                "Missing or malformed MCP-Protocol-Version header",
            )),
            Some(v) if v != crate::LATEST_PROTOCOL_VERSION => Some(
                Error::new(
                    ErrorCode::UnsupportedProtocolVersion,
                    format!("Unsupported MCP protocol version: {v}"),
                )
                .with_data(serde_json::json!({
                    "supported": [crate::LATEST_PROTOCOL_VERSION],
                    "requested": v,
                })),
            ),
            Some(_) => None,
        };
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
            // A wrong version header outranks an unparseable body: the header
            // is wrong whatever the body turns out to say, and its `400` is
            // mandated. There is no id to correlate against here, which is
            // precisely the case where none exists to be had.
            #[cfg(not(feature = "legacy-spec"))]
            if let Some(err) = version_err {
                return PostPrep::Reply(build_json_response(
                    http::StatusCode::BAD_REQUEST,
                    id,
                    &Message::Response(Response::error(RequestId::Null, err)),
                ));
            }
            let resp = Response::error(RequestId::Null, Error::from(code));
            return PostPrep::Reply(build_json_response(
                http::StatusCode::OK,
                id,
                &Message::Response(resp),
            ));
        }
    };

    #[cfg(not(feature = "legacy-spec"))]
    if let Some(err) = version_err {
        return PostPrep::Reply(build_json_response(
            http::StatusCode::BAD_REQUEST,
            id,
            &reject_post(&msg, err),
        ));
    }

    // Every request's `_meta` must carry the fields MCP 2026-07-28 makes
    // mandatory, and the version it states must agree with the header the gate
    // above already validated. Batched requests are checked one by one:
    // wrapping a request in an array must not be a way around the gate it
    // would face on its own.
    //
    // One offender rejects the whole POST -- these are conformance failures,
    // not application errors, and the `400` the spec mandates for a header
    // mismatch cannot be applied to half a POST. So every request in it is
    // answered: the offenders with what is wrong with them, the rest with the
    // fact that the POST they rode in on was not processed. Their callers are
    // waiting on ids too.
    #[cfg(not(feature = "legacy-spec"))]
    {
        let invalid = match &msg {
            Message::Request(r) => request_meta_error(r).is_some(),
            Message::Batch(batch) => batch.iter().any(|env| match env {
                crate::types::MessageEnvelope::Request(r) => request_meta_error(r).is_some(),
                _ => false,
            }),
            _ => false,
        };

        if invalid {
            let reply = reject_post_each(&msg, |r| {
                request_meta_error(r).unwrap_or_else(|| {
                    Error::new(
                        ErrorCode::InvalidRequest,
                        "Not processed: another request in this batch was rejected",
                    )
                })
            });

            if let Some(reply) = reply {
                return PostPrep::Reply(build_json_response(
                    http::StatusCode::BAD_REQUEST,
                    id,
                    &reply,
                ));
            }
        }
    }

    // The routing headers must describe the body they arrived with. An
    // intermediary is entitled to route or police on `Mcp-Method` / `Mcp-Name`
    // without parsing the body, so a server that dispatches a body naming a
    // different tool than its headers do turns those headers into a bypass.
    #[cfg(not(feature = "legacy-spec"))]
    {
        let invalid = match &msg {
            Message::Request(r) => routing_header_error(r, &headers)
                .map(|err| Message::Response(Response::error(r.id(), err))),
            // A batch has no single method or name for a header to mirror, so a
            // conforming client sends neither. One that arrives anyway cannot
            // have been derived from this body -- and an intermediary that
            // acted on it was answering about a request that is not in here.
            // The header was wrong for the whole batch, so the whole batch
            // hears about it.
            Message::Batch(_) => (headers.contains_key(crate::transport::http::MCP_METHOD)
                || headers.contains_key(crate::transport::http::MCP_NAME))
            .then(|| {
                reject_post(
                    &msg,
                    Error::new(
                        ErrorCode::HeaderMismatch,
                        "Mcp-Method / Mcp-Name cannot describe a batch and must be omitted",
                    ),
                )
            }),
            // The spec requires `Mcp-Method` on requests, so a notification
            // that omits it is conforming and is left alone. One that *states*
            // a method has to state its own: clients do send it here, so an
            // intermediary policing by `Mcp-Method` sees it, and a body saying
            // otherwise is exactly the bypass the request path is guarded
            // against. A notification has no id, so nothing is addressed.
            Message::Notification(n) => headers
                .get(crate::transport::http::MCP_METHOD)
                .and_then(|v| v.to_str().ok())
                .filter(|stated| *stated != n.method.as_str())
                .map(|stated| {
                    Message::Response(Response::error(
                        RequestId::Null,
                        Error::new(
                            ErrorCode::HeaderMismatch,
                            format!(
                                "Header mismatch: Mcp-Method header value {stated:?} \
                                 does not match body value {:?}",
                                n.method
                            ),
                        ),
                    ))
                }),
            _ => None,
        };

        if let Some(reply) = invalid {
            return PostPrep::Reply(build_json_response(
                http::StatusCode::BAD_REQUEST,
                id,
                &reply,
            ));
        }
    }

    // Passive W3C Trace Context recorder: when both MCP 2026-07-28
    // and `tracing` are enabled, record any `_meta.traceparent` /
    // `_meta.tracestate` / `_meta.baggage` on the active span.
    // `Span::current().record(...)` is a no-op unless the caller's span
    // declares these fields via
    // `#[instrument(fields(traceparent, tracestate, baggage))]`.
    #[cfg(all(not(feature = "legacy-spec"), feature = "tracing"))]
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
        if let Some(bg) = meta.get("baggage").and_then(|v| v.as_str()) {
            tracing::Span::current().record("baggage", bg);
        }
    }

    // Pre-register on the initialize handshake so the server can emit
    // events between the init POST response and the SSE GET. Stateless 2026-07-28
    // transport has no SSE GET, so this is skipped under the flag.
    #[cfg(feature = "legacy-spec")]
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

/// The 2026-07-28 arm of [`dispatch_post`]: a POST pipeline that can return a
/// request-scoped SSE response, mirroring [`handle_get_sse`].
///
/// When the request opts into request-scoped notifications (carries
/// `io.modelcontextprotocol/logLevel` or a `progressToken` in `_meta`), the
/// reply is an SSE stream: notifications produced while handling the request
/// flow first (routed via the per-request sink), then the final response closes
/// the stream. Otherwise the reply is a single JSON object (`Complete`),
/// exactly as [`handle_post`].
#[cfg(not(feature = "legacy-spec"))]
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
                        dispatched_status(&resp),
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
            let hold_for_ack = is_subscription_stream(&msg);
            let notif_rx = crate::types::notification::sink::register(
                id,
                ctx.sse_log_queue_capacity,
                hold_for_ack,
            )
            .await;

            if ctx.inbound_tx.send(Ok(msg)).await.is_err() {
                crate::types::notification::sink::unregister(&id);
                ctx.pending.remove(&full_id);
                return StreamResponse::Complete(status_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    id,
                ));
            }

            let stream = post_notification_stream(
                id,
                full_id,
                ctx.pending.clone(),
                notif_rx,
                resp_rx,
                hold_for_ack,
                ctx.sse_log_queue_capacity,
            )
            .map(|msg| E::ephemeral_event(&msg));

            StreamResponse::Stream {
                headers: HeaderMap::new(),
                stream,
            }
        }
    }
}

/// Whether a message opts into notifications on its own `POST` response
/// stream: a `subscriptions/listen`, or a request -- or, for a batch, *any*
/// contained request -- carrying `logLevel` or `progressToken` in `_meta`.
///
/// Batches count because a client (e.g. via `Client::apply_client_meta_to_batch`)
/// stamps the configured level onto every batched request; the inner requests
/// share this POST's session id (copied in `execute_batch`), so their
/// notifications route to the one sink and stream on this single response.
#[cfg(not(feature = "legacy-spec"))]
fn opts_into_notifications(msg: &Message) -> bool {
    match msg {
        Message::Request(r) => request_opts_in(r),
        Message::Batch(batch) => batch.iter().any(
            |env| matches!(env, crate::types::MessageEnvelope::Request(r) if request_opts_in(r)),
        ),
        _ => false,
    }
}

/// Whether this `POST` body *is* a subscription stream rather than a request's
/// own notification stream.
///
/// The distinction decides what may be written to it: a subscription stream
/// opens with the acknowledgment and carries that subscription's notifications,
/// so request-scoped log messages stay off it.
///
/// A batch counts if it contains a listen at all. neva's own client refuses to
/// batch one -- a batch slot has no handle to end the subscription with -- but
/// this server accepts what any peer sends, and a batched listen streams on
/// this same body with the same ordering requirement.
#[cfg(not(feature = "legacy-spec"))]
fn is_subscription_stream(msg: &Message) -> bool {
    fn is_listen(req: &crate::types::Request) -> bool {
        req.method == crate::types::subscription::commands::LISTEN
    }

    match msg {
        Message::Request(r) => is_listen(r),
        Message::Batch(batch) => batch
            .iter()
            .any(|env| matches!(env, crate::types::MessageEnvelope::Request(r) if is_listen(r))),
        _ => false,
    }
}

/// Whether a single request needs the streaming reply: `subscriptions/listen`
/// (whose whole point is a long-lived notification stream), or a request
/// carrying `logLevel` or `progressToken` in `_meta`.
#[cfg(not(feature = "legacy-spec"))]
fn request_opts_in(req: &crate::types::Request) -> bool {
    if req.method == crate::types::subscription::commands::LISTEN {
        return true;
    }
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
///
/// `hold_for_ack` marks a body that carries a `subscriptions/listen`: there the
/// acknowledgment MUST be the first message, and middleware logging ahead of
/// `next(ctx)` is queued before `Context::listen` ever runs. Anything arriving
/// before the acknowledgment is therefore held back and released right after
/// it -- ordering the stream rather than dropping the messages, which matters
/// because a mixed batch's other requests log here too, and their logs were
/// explicitly asked for. `hold_limit` bounds that buffer at what the sink
/// itself would have held; overflow past it is dropped rather than released,
/// because the acknowledgment coming first is the requirement and the logs
/// riding along are the accommodation.
#[cfg(not(feature = "legacy-spec"))]
fn post_notification_stream(
    id: uuid::Uuid,
    full_id: RequestId,
    pending: super::context::RequestMap,
    notif_rx: tokio::sync::mpsc::Receiver<Message>,
    resp_rx: tokio::sync::oneshot::Receiver<Message>,
    hold_for_ack: bool,
    hold_limit: usize,
) -> impl Stream<Item = Message> + Send {
    struct Cleanup {
        id: uuid::Uuid,
        full_id: RequestId,
        pending: super::context::RequestMap,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            crate::types::notification::sink::unregister(&self.id);
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
        /// Whether this is a subscription body whose acknowledgment has not
        /// gone out yet.
        awaiting_ack: bool,
        /// Messages that arrived before the acknowledgment, in order.
        held: std::collections::VecDeque<Message>,
        /// How many of those to hold before giving up on ordering.
        hold_limit: usize,
        /// Ready to emit, ahead of the channels.
        out: std::collections::VecDeque<Message>,
    }

    /// Whether a message is the acknowledgment that opens a subscription.
    fn is_ack(msg: &Message) -> bool {
        matches!(msg, Message::Notification(n)
            if n.method == crate::types::subscription::commands::ACKNOWLEDGED)
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
        awaiting_ack: hold_for_ack,
        held: std::collections::VecDeque::new(),
        hold_limit,
        out: std::collections::VecDeque::new(),
    };

    stream::unfold(state, |mut state| async move {
        // Anything already released goes out before either channel is polled.
        if let Some(msg) = state.out.pop_front() {
            return Some((msg, state));
        }

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
                Step::Notification(n) if state.awaiting_ack => {
                    if is_ack(&n) {
                        // The stream is open: the acknowledgment goes out now,
                        // and what was waiting on it follows.
                        state.awaiting_ack = false;
                        state.out.append(&mut state.held);
                        return Some((n, state));
                    }
                    // Bounded, so a handler that logs without end cannot grow
                    // this: past the sink's own capacity the overflow is
                    // dropped, exactly as the sink would have dropped it had
                    // nothing been draining it. Releasing it instead would put
                    // these messages ahead of the acknowledgment, and the
                    // acknowledgment coming first is the requirement -- the
                    // logs riding along are the accommodation.
                    if state.held.len() < state.hold_limit {
                        state.held.push_back(n);
                    } else {
                        #[cfg(feature = "tracing")]
                        tracing::warn!(
                            logger = "neva",
                            "dropped a notification queued before the subscription \
                             acknowledgment: the pre-acknowledgment buffer is full"
                        );
                    }
                }
                Step::Notification(n) => return Some((n, state)),
                Step::NotificationsClosed => {
                    state.notifs_open = false;
                    state.awaiting_ack = false;
                }
                Step::Response(Some(resp)) => {
                    // Buffer it and keep draining until the pipeline is done.
                    state.response = Some(resp);
                    state.resp_rx = None;
                    // The listen was answered without ever acknowledging --
                    // rejected, most likely. Nothing is waiting on an
                    // acknowledgment that is not coming.
                    state.awaiting_ack = false;
                    state.out.append(&mut state.held);
                }
                // The response channel was dropped: the runtime will never
                // answer, so stop waiting on notifications and end the body
                // rather than holding the connection open.
                Step::Response(None) => {
                    state.resp_rx = None;
                    state.notifs_open = false;
                    state.awaiting_ack = false;
                }
            }
        }

        // Whatever was still held has nowhere left to wait: release it ahead of
        // the response.
        state.out.append(&mut state.held);
        if let Some(msg) = state.out.pop_front() {
            return Some((msg, state));
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
    #[cfg_attr(not(feature = "legacy-spec"), allow(unused_variables))] headers: &HeaderMap,
) -> uuid::Uuid {
    // 2026-07-28 removed protocol-level sessions and the `Mcp-Session-Id` header, and
    // this id doubles as the per-POST correlation key for the pending-response
    // slot and the request notification sink. Mint a fresh one per POST so a
    // client-supplied (or proxied) header can never collide two concurrent
    // stateless requests onto the same sink/slot.
    #[cfg(not(feature = "legacy-spec"))]
    {
        uuid::Uuid::new_v4()
    }
    #[cfg(feature = "legacy-spec")]
    headers
        .get(MCP_SESSION_ID)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .unwrap_or_else(uuid::Uuid::new_v4)
}

/// Addresses a whole-POST rejection to every request the POST carried, each
/// with the verdict on itself.
///
/// A JSON-RPC error reaches its caller by id: the client resolves the pending
/// request whose id the reply names, and nothing else. A reply carrying `null`
/// therefore matches nothing -- the caller keeps waiting until it times out,
/// and the error it was handed is never seen, which on a version mismatch
/// costs it the one message that says what to do instead. A batch gets one
/// error per request for the same reason: the client registered a slot for
/// each of them, and answering only the offender leaves the rest of the batch
/// hanging on a POST that has already been decided.
///
/// `None` when the body carried no request at all -- a notification is never
/// answered, rejection included.
#[cfg(not(feature = "legacy-spec"))]
fn reject_post_each(
    msg: &Message,
    verdict: impl Fn(&crate::types::Request) -> Error,
) -> Option<Message> {
    use crate::types::{MessageBatch, MessageEnvelope};

    match msg {
        Message::Request(req) => Some(Message::Response(Response::error(req.id(), verdict(req)))),
        Message::Batch(batch) => {
            let items = batch
                .iter()
                .filter_map(|env| match env {
                    MessageEnvelope::Request(req) => Some(MessageEnvelope::Response(
                        Response::error(req.id(), verdict(req)),
                    )),
                    _ => None,
                })
                .collect::<Vec<_>>();

            // Fails only on an empty vec, i.e. a batch of notifications
            // throughout -- and then there is no one to answer.
            MessageBatch::new(items).map(Message::Batch).ok()
        }
        _ => None,
    }
}

/// [`reject_post_each`] for a failure the whole POST shares -- a transport
/// header that was wrong for every request underneath it -- where the verdict
/// on each request is the same one.
///
/// Falls back to an unaddressed reply when there is no request to address:
/// the POST is still answered, since the status is what carries the rejection.
#[cfg(not(feature = "legacy-spec"))]
fn reject_post(msg: &Message, err: Error) -> Message {
    // `Error` is not `Clone` -- its cause is a boxed `dyn StdError` -- and a
    // batch needs one reply per request, all saying the same thing.
    let restated = reject_post_each(msg, |_| {
        let copy = Error::new(err.code, err.to_string());
        match err.data() {
            Some(data) => copy.with_data(data.clone()),
            None => copy,
        }
    });

    restated.unwrap_or_else(|| Message::Response(Response::error(RequestId::Null, err)))
}

/// Why a request's `_meta` is unacceptable, if it is.
///
/// Two rules, in order. MCP 2026-07-28 makes `protocolVersion` (a string) and
/// `clientCapabilities` (an object) mandatory on every request -- capabilities
/// are declared per request precisely so a stateless server never has to infer
/// them from earlier traffic. A request that omits either, or states it with
/// the wrong JSON type, is malformed params (`-32602`): a version that is not
/// a string is not a version, and treating it as absent would let the next
/// rule be skipped by sending a number. A version that *is* stated must be one
/// this build serves, or it is `UnsupportedProtocolVersion` (`-32022`) naming
/// what is on offer.
///
/// Both rules belong to the message rather than to HTTP, so both live on
/// [`crate::types::Request`] and are enforced again at the dispatch seam for
/// the transports that have no preamble of their own. Catching them here is
/// what earns them the `400` the spec mandates on this one; the caller
/// supplies the status.
///
/// The header carrying the version was checked before the body was read, and
/// only this version passes that gate -- so a stated version that disagrees
/// with the header is exactly one this build does not serve, and the second
/// rule already covers it.
#[cfg(not(feature = "legacy-spec"))]
fn request_meta_error(req: &crate::types::Request) -> Option<Error> {
    req.required_meta_error()
        .or_else(|| req.unsupported_version_error())
}

/// The body value `Mcp-Name` mirrors for `req`, if its method has one.
///
/// The spec requires the header on `tools/call`, `resources/read` and
/// `prompts/get`; the Tasks extension adds `params.taskId` on its own methods.
/// A method with no source here has nothing for the header to disagree with.
#[cfg(not(feature = "legacy-spec"))]
fn name_source(req: &crate::types::Request) -> Option<(&str, bool)> {
    #[cfg(feature = "tasks")]
    {
        use crate::types::task::commands as tasks;
        if matches!(
            req.method.as_str(),
            tasks::GET | tasks::UPDATE | tasks::CANCEL
        ) {
            // The extension defines the header but the core spec does not
            // require it, so it is checked when sent and not demanded.
            let raw = req.params.as_ref()?.as_object()?.get("taskId")?.as_str()?;
            return Some((raw, false));
        }
    }

    let field = match req.method.as_str() {
        crate::types::tool::commands::CALL | crate::types::prompt::commands::GET => "name",
        crate::types::resource::commands::READ => "uri",
        _ => return None,
    };
    let raw = req.params.as_ref()?.as_object()?.get(field)?.as_str()?;
    Some((raw, true))
}

/// Why a request's routing headers do not describe its body, if they do not.
///
/// `Mcp-Method` is required on every request; `Mcp-Name` on the three methods
/// that name what they act on. Both must equal the body value they mirror,
/// after decoding the Base64 sentinel -- a value that claims that encoding and
/// does not honor it is rejected rather than compared raw.
#[cfg(not(feature = "legacy-spec"))]
fn routing_header_error(req: &crate::types::Request, headers: &HeaderMap) -> Option<Error> {
    let mismatch = |header: &str, stated: &str, body: &str| {
        Some(Error::new(
            ErrorCode::HeaderMismatch,
            format!(
                "Header mismatch: {header} header value {stated:?} does not match body value {body:?}"
            ),
        ))
    };
    let missing = |header: &str| {
        Some(Error::new(
            ErrorCode::HeaderMismatch,
            format!("Missing or malformed {header} header"),
        ))
    };

    let method = crate::transport::http::MCP_METHOD;
    match headers.get(method).and_then(|v| v.to_str().ok()) {
        None => return missing(method),
        Some(stated) if stated != req.method.as_str() => {
            return mismatch(method, stated, &req.method);
        }
        Some(_) => {}
    }

    let name = crate::transport::http::MCP_NAME;
    let stated = headers.get(name).and_then(|v| v.to_str().ok());
    match (name_source(req), stated) {
        (Some((_, true)), None) => missing(name),
        (Some((body, _)), Some(stated)) => {
            match crate::transport::http::decode_header_value(stated) {
                Some(decoded) if decoded == body => None,
                Some(decoded) => mismatch(name, &decoded, body),
                None => missing(name),
            }
        }
        _ => None,
    }
}

/// The HTTP status a dispatched JSON-RPC reply must be sent with.
///
/// Most application-level errors ride on `200 OK` -- JSON-RPC carries them in
/// the body. The MCP-allocated protocol errors are the exception: the spec
/// pins each of them to `400 Bad Request`, because they say the *request* was
/// wrong, not that the method failed. `MissingRequiredClientCapability` is
/// raised during dispatch rather than in the transport preamble, so the status
/// has to be recovered here from the reply.
///
/// A batch keeps `200 OK` even when some of its items carry such an error: one
/// status covers every item, and the per-item codes are in the body.
#[cfg(not(feature = "legacy-spec"))]
fn dispatched_status(msg: &Message) -> http::StatusCode {
    match msg {
        Message::Response(Response::Err(err)) => match err.error.code {
            ErrorCode::HeaderMismatch
            | ErrorCode::MissingRequiredClientCapability
            | ErrorCode::UnsupportedProtocolVersion => http::StatusCode::BAD_REQUEST,
            _ => http::StatusCode::OK,
        },
        _ => http::StatusCode::OK,
    }
}

/// The HTTP status a dispatched JSON-RPC reply must be sent with.
///
/// The legacy profile has no status-bearing error codes: every dispatched
/// reply is a `200 OK` with the error in the body.
#[cfg(feature = "legacy-spec")]
fn dispatched_status(_msg: &Message) -> http::StatusCode {
    http::StatusCode::OK
}

fn build_json_response(
    status: http::StatusCode,
    #[cfg_attr(not(feature = "legacy-spec"), allow(unused_variables))] session: uuid::Uuid,
    body: &Message,
) -> HttpResponse {
    let json = serde_json::to_vec(body).unwrap_or_default();
    #[cfg_attr(not(feature = "legacy-spec"), allow(unused_mut))]
    let mut resp = http::Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Bytes::from(json))
        .unwrap_or_default();
    // Stateless 2026-07-28 transport never puts the session id on the wire.
    #[cfg(feature = "legacy-spec")]
    if let Ok(v) = HeaderValue::from_str(&session.to_string()) {
        resp.headers_mut().insert(MCP_SESSION_ID, v);
    }
    resp
}

fn status_response(
    status: http::StatusCode,
    #[cfg_attr(not(feature = "legacy-spec"), allow(unused_variables))] session: uuid::Uuid,
) -> HttpResponse {
    #[cfg_attr(not(feature = "legacy-spec"), allow(unused_mut))]
    let mut resp = http::Response::builder()
        .status(status)
        .body(Bytes::new())
        .unwrap_or_default();
    // Stateless 2026-07-28 transport never puts the session id on the wire.
    #[cfg(feature = "legacy-spec")]
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

    /// The `_meta` MCP 2026-07-28 requires on every request. Empty
    /// capabilities are a valid declaration -- "no optional capabilities" --
    /// which is what a bare test request means.
    #[cfg(not(feature = "legacy-spec"))]
    fn meta() -> serde_json::Value {
        serde_json::json!({
            "io.modelcontextprotocol/protocolVersion": crate::LATEST_PROTOCOL_VERSION,
            "io.modelcontextprotocol/clientCapabilities": {}
        })
    }

    fn make_request_body(method: &str) -> Bytes {
        #[cfg(not(feature = "legacy-spec"))]
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "id": 1,
            "params": { "_meta": meta() }
        });
        #[cfg(feature = "legacy-spec")]
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

    /// A POST request builder that, under MCP 2026-07-28, carries the required
    /// `MCP-Protocol-Version` header so it passes the stateless gate.
    fn post_builder() -> http::request::Builder {
        let b = http::Request::builder().method("POST").uri("/mcp");
        #[cfg(not(feature = "legacy-spec"))]
        let b = b.header(crate::transport::http::MCP_PROTOCOL_VERSION, "2026-07-28");
        b
    }

    /// [`post_builder`] plus the `Mcp-Method` routing header, for the tests
    /// whose body is a request the server is expected to dispatch.
    fn post_builder_for(method: &str) -> http::request::Builder {
        let b = post_builder();
        #[cfg(not(feature = "legacy-spec"))]
        let b = b.header(crate::transport::http::MCP_METHOD, method);
        #[cfg(feature = "legacy-spec")]
        let _ = method;
        b
    }

    #[cfg(not(feature = "legacy-spec"))]
    #[tokio::test]
    async fn rejects_missing_protocol_version() {
        let (ctx, _rx) = make_ctx();
        let req = http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .body(make_request_body("ping"))
            .unwrap();
        let resp = handle_post(req, &ctx).await;
        // A missing header is a header problem, and the spec mandates 400.
        assert_eq!(resp.status(), http::StatusCode::BAD_REQUEST);
        let body: serde_json::Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(body["error"]["code"], -32020);
        // Addressed to the request that was rejected: a reply the client
        // cannot correlate is one it waits out instead of reading.
        assert_eq!(body["id"], 1);
    }

    #[cfg(not(feature = "legacy-spec"))]
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
        // A well-formed header naming a version we do not speak is a version
        // problem, and the client is told what is on offer so it can retry.
        assert_eq!(resp.status(), http::StatusCode::BAD_REQUEST);
        let body: serde_json::Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(body["error"]["code"], -32022);
        assert_eq!(body["id"], 1);
        assert_eq!(body["error"]["data"]["requested"], "1999-01-01");
        assert_eq!(
            body["error"]["data"]["supported"],
            serde_json::json!(["2026-07-28"])
        );
    }

    /// A batch shares the header that was wrong, so every request under it is
    /// answered -- each client slot is waiting on its own id.
    #[cfg(not(feature = "legacy-spec"))]
    #[tokio::test]
    async fn rejects_every_request_of_a_batch_on_a_bad_version() {
        let (ctx, _rx) = make_ctx();
        let body = serde_json::json!([
            { "jsonrpc": "2.0", "method": "ping", "id": 1, "params": { "_meta": meta() } },
            { "jsonrpc": "2.0", "method": "notifications/initialized" },
            { "jsonrpc": "2.0", "method": "ping", "id": 2, "params": { "_meta": meta() } },
        ]);
        let req = http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(crate::transport::http::MCP_PROTOCOL_VERSION, "1999-01-01")
            .body(Bytes::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        let resp = handle_post(req, &ctx).await;
        assert_eq!(resp.status(), http::StatusCode::BAD_REQUEST);
        let body: serde_json::Value = serde_json::from_slice(resp.body()).unwrap();
        let items = body.as_array().expect("a batch is answered with a batch");
        // Two requests, two errors -- and nothing for the notification.
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["id"], 1);
        assert_eq!(items[1]["id"], 2);
        for item in items {
            assert_eq!(item["error"]["code"], -32022);
            assert_eq!(item["error"]["data"]["requested"], "1999-01-01");
        }
    }

    /// A body that never parsed has no id to answer to, but the header was
    /// still wrong -- and its status is the mandated one, not the parse
    /// error's.
    #[cfg(not(feature = "legacy-spec"))]
    #[tokio::test]
    async fn a_bad_version_outranks_an_unparseable_body() {
        let (ctx, _rx) = make_ctx();
        let req = http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(crate::transport::http::MCP_PROTOCOL_VERSION, "1999-01-01")
            .body(Bytes::from_static(b"{ not json"))
            .unwrap();

        let resp = handle_post(req, &ctx).await;
        assert_eq!(resp.status(), http::StatusCode::BAD_REQUEST);
        let body: serde_json::Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(body["error"]["code"], -32022);
        assert!(body["id"].is_null());
    }

    #[cfg(not(feature = "legacy-spec"))]
    #[tokio::test]
    async fn rejects_legacy_protocol_version() {
        // A legacy version that IS in `PROTOCOL_VERSIONS` but is not the 2026-07-28
        // version: the old `.contains()` gate accepted it even though this build
        // only speaks 2026-07-28 stateless semantics. It must be rejected.
        let (ctx, _rx) = make_ctx();
        let req = http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(crate::transport::http::MCP_PROTOCOL_VERSION, "2025-06-18")
            .body(make_request_body("ping"))
            .unwrap();
        let resp = handle_post(req, &ctx).await;
        assert_eq!(resp.status(), http::StatusCode::BAD_REQUEST);
        let body: serde_json::Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(body["error"]["code"], -32022);
    }

    /// The body states the version the request is made under, and a version
    /// this build does not serve is refused there too -- putting it past the
    /// header gate does not make it servable.
    #[cfg(not(feature = "legacy-spec"))]
    #[tokio::test]
    async fn rejects_a_body_version_this_build_does_not_serve() {
        let (ctx, _rx) = make_ctx();
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list",
            "params": { "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2025-06-18",
                "io.modelcontextprotocol/clientCapabilities": {}
            } }
        });
        let req = post_builder()
            .body(bytes::Bytes::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = handle_post(req, &ctx).await;
        assert_eq!(resp.status(), http::StatusCode::BAD_REQUEST);
        let body: serde_json::Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(body["error"]["code"], -32022);
        // Told what is on offer, so it can retry rather than guess.
        assert_eq!(body["error"]["data"]["requested"], "2025-06-18");
        assert_eq!(
            body["error"]["data"]["supported"],
            serde_json::json!(["2026-07-28"])
        );
    }

    /// `protocolVersion` and `clientCapabilities` are required on every
    /// request -- capabilities per request, so a stateless server never infers
    /// them from earlier traffic. Missing either is malformed params.
    #[cfg(not(feature = "legacy-spec"))]
    #[tokio::test]
    async fn rejects_a_request_missing_required_meta() {
        let cases = [
            // No `params` at all.
            serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
            // `params`, but no `_meta`.
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}
            }),
            // Capabilities without a version.
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/list",
                "params": { "_meta": { "io.modelcontextprotocol/clientCapabilities": {} } }
            }),
            // A version without capabilities.
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/list",
                "params": { "_meta": {
                    "io.modelcontextprotocol/protocolVersion": crate::LATEST_PROTOCOL_VERSION
                } }
            }),
            // Present, but not a string -- it cannot be compared against the
            // header, and must not be a way past the mismatch check either.
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/list",
                "params": { "_meta": {
                    "io.modelcontextprotocol/protocolVersion": 20260728,
                    "io.modelcontextprotocol/clientCapabilities": {}
                } }
            }),
            // Capabilities that are not an object declare nothing.
            serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/list",
                "params": { "_meta": {
                    "io.modelcontextprotocol/protocolVersion": crate::LATEST_PROTOCOL_VERSION,
                    "io.modelcontextprotocol/clientCapabilities": "elicitation"
                } }
            }),
        ];

        for case in cases {
            let (ctx, _rx) = make_ctx();
            let req = post_builder()
                .body(bytes::Bytes::from(serde_json::to_vec(&case).unwrap()))
                .unwrap();
            let resp = handle_post(req, &ctx).await;
            assert_eq!(
                resp.status(),
                http::StatusCode::BAD_REQUEST,
                "must answer 400: {case}"
            );
            let body: serde_json::Value = serde_json::from_slice(resp.body()).unwrap();
            assert_eq!(body["error"]["code"], -32602, "must be malformed params");
        }
    }

    /// Empty capabilities are a declaration, not an omission.
    #[cfg(not(feature = "legacy-spec"))]
    #[tokio::test]
    async fn accepts_a_request_declaring_no_capabilities() {
        let (ctx, _rx) = make_ctx();
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list",
            "params": { "_meta": meta() }
        });
        let req = post_builder_for("tools/list")
            .body(bytes::Bytes::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let ctx = std::sync::Arc::new(ctx);
        let ctx_clone = ctx.clone();
        let _h = tokio::spawn(async move { handle_post(req, &ctx_clone).await });

        // Not rejected in the preamble: it reaches dispatch and parks on its
        // pending slot, which nothing in this test answers.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(ctx.pending.len(), 1);
    }

    /// An intermediary may route or police on `Mcp-Method` / `Mcp-Name` without
    /// parsing the body. A server that dispatches a body those headers do not
    /// describe turns them into a bypass, so header and body must agree.
    #[cfg(not(feature = "legacy-spec"))]
    #[tokio::test]
    async fn rejects_routing_headers_that_do_not_describe_the_body() {
        let call = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "safe_tool", "arguments": {}, "_meta": meta() }
        });

        // (header name, header value) pairs to send instead of the honest ones.
        let cases: Vec<Vec<(&str, &str)>> = vec![
            // The body invokes a tool the headers do not name -- the bypass.
            vec![("Mcp-Method", "tools/call"), ("Mcp-Name", "allowed_tool")],
            // The body's method is not the one an intermediary was shown.
            vec![("Mcp-Method", "tools/list"), ("Mcp-Name", "safe_tool")],
            // Required headers missing outright.
            vec![("Mcp-Name", "safe_tool")],
            vec![("Mcp-Method", "tools/call")],
            vec![],
            // Sentinel claimed but not honored: not decodable, so not comparable.
            vec![("Mcp-Method", "tools/call"), ("Mcp-Name", "=?base64?%%%?=")],
        ];

        for case in cases {
            let (ctx, _rx) = make_ctx();
            let mut req = post_builder();
            for (name, value) in &case {
                req = req.header(*name, *value);
            }
            let req = req
                .body(bytes::Bytes::from(serde_json::to_vec(&call).unwrap()))
                .unwrap();

            let resp = handle_post(req, &ctx).await;
            assert_eq!(
                resp.status(),
                http::StatusCode::BAD_REQUEST,
                "must answer 400: {case:?}"
            );
            let body: serde_json::Value = serde_json::from_slice(resp.body()).unwrap();
            assert_eq!(body["error"]["code"], -32020, "must be a header mismatch");
        }
    }

    /// A name that cannot ride as a plain header value travels Base64-encoded,
    /// and the server compares what it decodes -- not the sentinel.
    #[cfg(not(feature = "legacy-spec"))]
    #[tokio::test]
    async fn accepts_a_base64_encoded_name_matching_the_body() {
        let (ctx, _rx) = make_ctx();
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "resources/read",
            "params": { "uri": "file:///café.txt", "_meta": meta() }
        });
        let req = post_builder()
            .header(crate::transport::http::MCP_METHOD, "resources/read")
            // Spelled out rather than produced by the encoder: this asserts
            // what the server accepts off the wire, not that the two helpers
            // agree with each other.
            .header(
                crate::transport::http::MCP_NAME,
                "=?base64?ZmlsZTovLy9jYWbDqS50eHQ=?=",
            )
            .body(bytes::Bytes::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        let ctx = std::sync::Arc::new(ctx);
        let ctx_clone = ctx.clone();
        let _h = tokio::spawn(async move { handle_post(req, &ctx_clone).await });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(ctx.pending.len(), 1, "the request must reach dispatch");
    }

    /// A notification is not required to carry `Mcp-Method`, but one that does
    /// is subject to the same agreement a request's is: clients send it, so an
    /// intermediary polices by it.
    #[cfg(not(feature = "legacy-spec"))]
    #[tokio::test]
    async fn rejects_a_notification_whose_method_header_disagrees() {
        let (ctx, _rx) = make_ctx();
        let req = post_builder()
            .header(crate::transport::http::MCP_METHOD, "notifications/progress")
            .body(make_notification_body("notifications/cancelled"))
            .unwrap();

        let resp = handle_post(req, &ctx).await;
        assert_eq!(resp.status(), http::StatusCode::BAD_REQUEST);
        let body: serde_json::Value = serde_json::from_slice(resp.body()).unwrap();
        assert_eq!(body["error"]["code"], -32020);
        assert!(body["id"].is_null(), "a notification has no id: {body}");
    }

    /// ...and one that omits the header is conforming, since the spec requires
    /// it on requests only.
    #[cfg(not(feature = "legacy-spec"))]
    #[tokio::test]
    async fn accepts_a_notification_without_a_method_header() {
        let (ctx, _rx) = make_ctx();
        let req = post_builder()
            .body(make_notification_body("notifications/cancelled"))
            .unwrap();

        let resp = handle_post(req, &ctx).await;
        assert_eq!(resp.status(), http::StatusCode::ACCEPTED);
    }

    /// No single method or name describes a batch, so a conforming client sends
    /// neither -- and one that arrives cannot have come from this body.
    #[cfg(not(feature = "legacy-spec"))]
    #[tokio::test]
    async fn rejects_routing_headers_on_a_batch() {
        let (ctx, _rx) = make_ctx();
        let body = serde_json::json!([
            { "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { "_meta": meta() } },
            {
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "evil", "arguments": {}, "_meta": meta() }
            }
        ]);
        let req = post_builder()
            .header(crate::transport::http::MCP_METHOD, "tools/list")
            .body(bytes::Bytes::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();

        let resp = handle_post(req, &ctx).await;
        assert_eq!(resp.status(), http::StatusCode::BAD_REQUEST);
        let body: serde_json::Value = serde_json::from_slice(resp.body()).unwrap();
        // The header was wrong for the whole batch, so every item in it is
        // told so -- each is a slot some caller is waiting on.
        let items = body.as_array().expect("a batch is answered with a batch");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["id"], 1);
        assert_eq!(items[1]["id"], 2);
        for item in items {
            assert_eq!(item["error"]["code"], -32020);
        }
    }

    /// A batch must not be a way around the version gate a standalone request
    /// faces: the offending item is caught while the array is still unopened.
    #[cfg(not(feature = "legacy-spec"))]
    #[tokio::test]
    async fn rejects_a_batched_body_version_this_build_does_not_serve() {
        let (ctx, _rx) = make_ctx();
        let body = serde_json::json!([
            { "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": { "_meta": meta() } },
            {
                "jsonrpc": "2.0", "id": 2, "method": "prompts/list",
                "params": { "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2025-06-18",
                    "io.modelcontextprotocol/clientCapabilities": {}
                } }
            }
        ]);
        let req = post_builder()
            .body(bytes::Bytes::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = handle_post(req, &ctx).await;
        assert_eq!(resp.status(), http::StatusCode::BAD_REQUEST);
        let body: serde_json::Value = serde_json::from_slice(resp.body()).unwrap();
        let items = body.as_array().expect("a batch is answered with a batch");
        assert_eq!(items.len(), 2);

        // The offender hears what is wrong with it...
        assert_eq!(items[1]["id"], 2);
        assert_eq!(items[1]["error"]["code"], -32022);
        // ...and the item that rode in with it hears that the POST carrying it
        // was not processed, rather than nothing at all.
        assert_eq!(items[0]["id"], 1);
        assert_eq!(items[0]["error"]["code"], -32600);
    }

    /// `-32021` is raised during dispatch rather than in the preamble, so the
    /// mandated `400` has to be recovered from the reply on its way out.
    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn spec_error_codes_map_to_400() {
        for code in [
            ErrorCode::HeaderMismatch,
            ErrorCode::MissingRequiredClientCapability,
            ErrorCode::UnsupportedProtocolVersion,
        ] {
            let msg = Message::Response(Response::error(
                RequestId::Number(1),
                Error::new(code, "nope"),
            ));
            assert_eq!(
                dispatched_status(&msg),
                http::StatusCode::BAD_REQUEST,
                "{code:?} must answer 400"
            );
        }

        // An ordinary application error still rides on `200 OK`.
        let msg = Message::Response(Response::error(
            RequestId::Number(1),
            Error::new(ErrorCode::InvalidParams, "nope"),
        ));
        assert_eq!(dispatched_status(&msg), http::StatusCode::OK);
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
        let req = post_builder_for(crate::commands::INIT)
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

    // Session-id echo is intentionally removed under the stateless 2026-07-28 transport.
    #[cfg(feature = "legacy-spec")]
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

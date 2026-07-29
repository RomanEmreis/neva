//! HTTP client implementation

use self::mcp_session::McpSession;
use crate::{
    error::{Error, ErrorCode},
    transport::http::{ClientRuntimeContext, MCP_SESSION_ID, get_mcp_session_id},
    types::Message,
};
use futures_util::{StreamExt, TryStreamExt};
use reqwest::header::{CACHE_CONTROL, HeaderName};
use reqwest::{
    RequestBuilder,
    header::{ACCEPT, CONTENT_TYPE},
};
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "client-tls")]
use tls_config::ClientTlsConfig;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub(super) mod mcp_session;
#[cfg(feature = "client-oauth")]
pub(crate) mod oauth;
#[cfg(feature = "client-tls")]
pub(crate) mod tls_config;

/// How outgoing requests are authenticated.
///
/// Built once per connection from the runtime context; cheap to clone
/// into every request task.
#[derive(Clone)]
enum ClientAuth {
    /// No credential attached.
    None,
    /// Static bearer token from `HttpClient::with_auth`.
    Static(Arc<str>),
    /// Managed OAuth session -- the token changes as flows complete.
    #[cfg(feature = "client-oauth")]
    OAuth(Arc<oauth::OAuthSession>),
}

impl ClientAuth {
    /// The bearer token to attach to the next request, if any. Under a
    /// managed OAuth session a token about to expire is refreshed first
    /// (non-interactive, when a refresh token is available).
    async fn fresh_bearer(&self) -> Option<Arc<str>> {
        match self {
            ClientAuth::None => None,
            ClientAuth::Static(token) => Some(token.clone()),
            #[cfg(feature = "client-oauth")]
            ClientAuth::OAuth(session) => session.refreshed_bearer().await,
        }
    }

    fn from_static(access_token: Option<Box<[u8]>>) -> Self {
        match access_token {
            Some(token) => ClientAuth::Static(String::from_utf8_lossy(&token).into()),
            None => ClientAuth::None,
        }
    }
}

// SSE constants -- the standalone GET stream serves legacy peers only;
// its machinery compiles under both flags for the dual-mode client and
// activates at runtime when a legacy `initialize` handshake happens.
const LAST_EVENT_ID: HeaderName = HeaderName::from_static("last-event-id");
const SSE_RECONNECT_DELAY: Duration = Duration::from_secs(3);
const STREAM_ENDED_BEFORE_RESPONSE: &str = "POST SSE stream ended before the response arrived";

#[cfg(not(feature = "legacy-spec"))]
fn routing_hints(msg: &Message) -> Option<(&str, Option<String>)> {
    match msg {
        Message::Request(r) => Some((r.method.as_str(), name_param(r))),
        Message::Notification(n) => Some((n.method.as_str(), None)),
        Message::Batch(_) | Message::Response(_) => None,
    }
}

/// The `Mcp-Name` value for `req`, already header-encoded.
///
/// The spec requires the header on `tools/call`, `resources/read` and
/// `prompts/get` (sourced from `params.name` / `params.uri`); the Tasks
/// extension adds `params.taskId` on its own methods so an intermediary can
/// route every call for a task to the instance holding its state.
#[cfg(not(feature = "legacy-spec"))]
fn name_param(req: &crate::types::Request) -> Option<String> {
    #[cfg(feature = "tasks")]
    {
        use crate::types::task::commands as tasks;
        if matches!(
            req.method.as_str(),
            tasks::GET | tasks::UPDATE | tasks::CANCEL
        ) {
            let raw = req.params.as_ref()?.as_object()?.get("taskId")?.as_str()?;
            return Some(crate::transport::http::encode_header_value(raw));
        }
    }

    let field = match req.method.as_str() {
        crate::types::tool::commands::CALL | crate::types::prompt::commands::GET => "name",
        crate::types::resource::commands::READ => "uri",
        _ => return None,
    };

    let raw = req.params.as_ref()?.as_object()?.get(field)?.as_str()?;

    Some(crate::transport::http::encode_header_value(raw))
}

/// The `Mcp-Param-*` headers a `tools/call` mirrors, per the called tool's
/// registered `x-mcp-header` annotations.
///
/// A batch mirrors nothing, for the same reason it carries no `Mcp-Method` or
/// `Mcp-Name`: one set of headers cannot describe several calls, and two
/// batched calls of the same tool would fight over one header name. Batching an
/// annotated call therefore hides it from header-based routing -- the servers
/// on the other end skip the matching check rather than reject it -- so a
/// caller that needs an intermediary to see a call should send it on its own.
#[cfg(not(feature = "legacy-spec"))]
fn param_headers(
    msg: &Message,
    registry: &crate::shared::param_headers::Registry,
) -> Vec<(String, String)> {
    let Message::Request(req) = msg else {
        return Vec::new();
    };
    if req.method != crate::types::tool::commands::CALL {
        return Vec::new();
    }
    let Some(params) = req.params.as_ref().and_then(|p| p.as_object()) else {
        return Vec::new();
    };
    let Some(name) = params.get("name").and_then(|n| n.as_str()) else {
        return Vec::new();
    };
    let Some(entry) = registry.get(name) else {
        return Vec::new();
    };
    let args = params.get("arguments").cloned().unwrap_or_default();
    crate::shared::param_headers::extract(entry.value(), &args)
}

pub(super) async fn connect(rt: ClientRuntimeContext, token: CancellationToken) {
    let session = Arc::new(McpSession::new(
        rt.url,
        token,
        #[cfg(not(feature = "legacy-spec"))]
        rt.peer_mode.clone(),
    ));

    #[cfg(feature = "client-oauth")]
    let auth = match rt.oauth {
        Some(oauth) => ClientAuth::OAuth(oauth),
        None => ClientAuth::from_static(rt.access_token),
    };
    #[cfg(not(feature = "client-oauth"))]
    let auth = ClientAuth::from_static(rt.access_token);

    // The SSE task arms itself only when a legacy `initialize` handshake
    // completes (`session.initialized()` fires exclusively for the
    // `initialize` method) -- against a 2026-07-28 peer it stays parked until
    // cancellation, so the stateless 2026-07-28 transport still issues only POSTs.
    tokio::join!(
        handle_connection(
            session.clone(),
            rt.rx,
            rt.tx.clone(),
            auth.clone(),
            #[cfg(not(feature = "legacy-spec"))]
            rt.param_headers.clone(),
            #[cfg(feature = "client-tls")]
            rt.tls_config.clone()
        ),
        start_sse_connection(
            session.clone(),
            rt.tx.clone(),
            auth.clone(),
            #[cfg(feature = "client-tls")]
            rt.tls_config.clone()
        )
    );
}

async fn handle_connection(
    session: Arc<McpSession>,
    mut sender_rx: mpsc::Receiver<Message>,
    recv_tx: mpsc::Sender<Result<Message, Error>>,
    auth: ClientAuth,
    #[cfg(not(feature = "legacy-spec"))] param_registry: crate::shared::param_headers::Registry,
    #[cfg(feature = "client-tls")] tls_config: Option<ClientTlsConfig>,
) {
    #[cfg(not(feature = "client-tls"))]
    let client = match create_client() {
        Ok(client) => client,
        Err(_err) => {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "HTTP client error: {_err:#}");
            return;
        }
    };

    #[cfg(feature = "client-tls")]
    let client = match create_client(tls_config) {
        Ok(client) => client,
        Err(_err) => {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "HTTP client error: {_err:#}");
            return;
        }
    };

    let token = session.cancellation_token();
    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => return,
            req = sender_rx.recv() => {
                let Some(req) = req else {
                    #[cfg(feature = "tracing")]
                    tracing::error!(logger = "neva", "Unexpected messaging error");
                    break;
                };
                crate::spawn_fair!(send_request(
                    client.clone(),
                    session.clone(),
                    req,
                    recv_tx.clone(),
                    auth.clone(),
                    #[cfg(not(feature = "legacy-spec"))]
                    param_registry.clone(),
                ));
            }
        }
    }
}

/// Builds the JSON-RPC POST with all transport headers and the current
/// bearer credential attached.
fn build_post(
    client: &reqwest::Client,
    session: &McpSession,
    req: &Message,
    bearer: Option<&str>,
    #[cfg(not(feature = "legacy-spec"))] param_registry: &crate::shared::param_headers::Registry,
) -> RequestBuilder {
    let mut resp = client
        .post(session.url())
        .json(req)
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream");

    if let Some(session_id) = session.session_id() {
        resp = resp.header(MCP_SESSION_ID, session_id.to_string())
    }

    // 2026-07-28-peer routing headers: legacy servers never negotiated them, so
    // a peer that fell back to `initialize` gets the same wire shape a
    // pure legacy client produces (no routing headers, no 2026-07-28 protocol
    // version). Routing headers are exercised end-to-end via the
    // trace-context integration; unit-level hint extraction is tested in
    // `routing_hints_tests`.
    #[cfg(not(feature = "legacy-spec"))]
    if !session.is_legacy() {
        if let Some((method, name)) = routing_hints(req) {
            resp = resp.header(crate::transport::http::MCP_METHOD, method);
            if let Some(n) = name {
                resp = resp.header(crate::transport::http::MCP_NAME, n);
            }
        }
        for (name, value) in param_headers(req, param_registry) {
            resp = resp.header(name, crate::transport::http::encode_header_value(&value));
        }
        resp = resp.header(
            crate::transport::http::MCP_PROTOCOL_VERSION,
            crate::LATEST_PROTOCOL_VERSION,
        );
    }

    if let Some(bearer) = bearer {
        resp = resp.bearer_auth(bearer)
    }
    resp
}

async fn send_request(
    client: reqwest::Client,
    session: Arc<McpSession>,
    req: Message,
    resp_tx: mpsc::Sender<Result<Message, Error>>,
    auth: ClientAuth,
    #[cfg(not(feature = "legacy-spec"))] param_registry: crate::shared::param_headers::Registry,
) {
    let bearer = auth.fresh_bearer().await;
    let resp = match build_post(
        &client,
        &session,
        &req,
        bearer.as_deref(),
        #[cfg(not(feature = "legacy-spec"))]
        &param_registry,
    )
    .send()
    .await
    {
        Ok(resp) => resp,
        Err(_err) => {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "Failed to send HTTP request: {}", _err);
            return;
        }
    };

    // A 401 under a managed OAuth session triggers the authorization
    // flow (single-flight across concurrent requests) and one retry with
    // the fresh token. On flow failure the original 401 falls through to
    // the regular response path.
    #[cfg(feature = "client-oauth")]
    let resp = match (&auth, resp.status()) {
        (ClientAuth::OAuth(oauth), reqwest::StatusCode::UNAUTHORIZED) => {
            let challenge = resp
                .headers()
                .get(reqwest::header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            match oauth
                .authorize(challenge.as_deref(), bearer.as_deref())
                .await
            {
                Ok(fresh) => {
                    match build_post(
                        &client,
                        &session,
                        &req,
                        Some(&fresh),
                        #[cfg(not(feature = "legacy-spec"))]
                        &param_registry,
                    )
                    .send()
                    .await
                    {
                        Ok(retried) => retried,
                        Err(_err) => {
                            #[cfg(feature = "tracing")]
                            tracing::error!(
                                logger = "neva",
                                "Failed to resend HTTP request: {}",
                                _err
                            );
                            return;
                        }
                    }
                }
                Err(_err) => {
                    #[cfg(feature = "tracing")]
                    tracing::error!(logger = "neva", "OAuth authorization failed: {}", _err);
                    resp
                }
            }
        }
        _ => resp,
    };

    if let Message::Notification(_) = &req {
        return;
    }

    // A notification-only batch also produces no server response (HTTP 202,
    // empty body). Attempting resp.json() on an empty body would be a parse
    // error that gets pushed into recv_tx and breaks the receive loop.
    if let Message::Batch(ref batch) = req
        && !batch.has_requests()
    {
        return;
    }

    if !session.has_session_id()
        && let Some(session_id) = get_mcp_session_id(resp.headers())
    {
        session.set_session_id(session_id);
    }

    if let Message::Request(r) = &req
        && r.method == crate::commands::INIT
    {
        let token = session.cancellation_token();
        session.notify_session_initialized();
        // Wait for the SSE GET to succeed. If it fails (non-2xx, network error) the
        // session is cancelled, which unblocks this select and aborts the init flow
        // rather than hanging forever.
        tokio::select! {
            biased;
            _ = token.cancelled() => return,
            _ = session.sse_ready() => {},
        }
    }

    let status = resp.status();

    // Streamable HTTP allows a POST reply to be a request-scoped SSE stream
    // (MCP 2026-07-28): it carries this request's `notifications/message` /
    // `notifications/progress` followed by the response. Forward every parsed
    // message to the receive loop, which routes notifications to handlers and
    // resolves the pending request on the response.
    if is_event_stream(resp.headers()) {
        let stream = sse_stream::SseStream::from_bytes_stream(resp.bytes_stream());
        let answered = drain_post_sse(stream, &resp_tx).await;

        // A truncated stream, an unparseable frame, or EOF before the final
        // response would otherwise leave the originating request sitting in the
        // pending queue until it times out. Fail it now, id-bound, exactly like
        // the non-JSON-RPC reply path below. `InternalError` (not `ParseError`)
        // on purpose: the peer clearly speaks 2026-07-28, so this must not be mistaken
        // for dual-mode fallback evidence.
        if !answered {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", STREAM_ENDED_BEFORE_RESPONSE);
            for id in request_ids(&req) {
                let resp = crate::types::Response::error(
                    id,
                    Error::new(ErrorCode::InternalError, STREAM_ENDED_BEFORE_RESPONSE),
                );
                if resp_tx.send(Ok(Message::Response(resp))).await.is_err() {
                    break;
                }
            }
        }
        return;
    }

    match resp.json::<Message>().await {
        Ok(msg) => {
            if let Err(_err) = resp_tx.send(Ok(msg)).await {
                #[cfg(feature = "tracing")]
                tracing::error!(logger = "neva", "Failed to send response: {}", _err);
            }
        }
        // A reply that is not JSON-RPC -- an HTML error page, or an error
        // code outside neva's `ErrorCode` set (e.g. the TS SDK's -32000).
        // Complete every originating request with an id-bound error
        // response: a bare `Err` pushed into the channel would terminate
        // the receive loop without ever resolving the pending request.
        // This is also what lets `server/discover` classify such replies
        // and fall back to `initialize`.
        Err(err) => {
            #[cfg(feature = "tracing")]
            tracing::error!(
                logger = "neva",
                "Failed to parse HTTP response ({}): {}",
                status,
                err
            );
            let (code, reason) = parse_failure(status, &err);
            for id in request_ids(&req) {
                let resp = crate::types::Response::error(id, Error::new(code, reason.clone()));
                if resp_tx.send(Ok(Message::Response(resp))).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// Classifies a non-JSON-RPC HTTP reply into the code and message the
/// originating requests are completed with, carrying the HTTP status so
/// the caller can tell *why* the body wasn't JSON-RPC.
///
/// The code matters beyond diagnostics: `ParseError` is one of the
/// dual-mode fallback triggers (issue #84), so it must be produced *only*
/// for replies that genuinely suggest "this peer doesn't know the 2026-07-28
/// method/route" -- an allowlist, not a catch-all:
///
/// * any `2xx` -- a legacy peer answering `server/discover` on the wire but
///   with a body neva can't read as JSON-RPC, most notably an error code
///   outside its `ErrorCode` set (the TS SDK's `-32000` "server not
///   initialized" family);
/// * `400` / `404` / `405` / `406` -- routers and legacy servers rejecting
///   the unknown method or endpoint outright.
///
/// Everything else is an upstream failure that says nothing about the
/// peer's protocol generation and must surface as-is
/// (`InternalError`, like "Connection closed"):
/// `401`/`403`/`407` (authentication -- otherwise a failed login against a
/// valid 2026-07-28 server reads as "legacy"), `429` (rate limit) and every `5xx`
/// (reverse-proxy outage, gateway timeout). Treating those as legacy
/// evidence would silently drop the 2026-07-28 headers, retry `initialize` into
/// the same outage, and bury the real cause.
#[inline]
fn parse_failure(status: reqwest::StatusCode, err: &impl std::fmt::Display) -> (ErrorCode, String) {
    let unsupported_route = matches!(status.as_u16(), 400 | 404 | 405 | 406);
    let code = if status.is_success() || unsupported_route {
        ErrorCode::ParseError
    } else {
        ErrorCode::InternalError
    };
    (code, format!("HTTP {status}: {err}"))
}

/// Whether a reply is SSE-framed rather than a single JSON document.
fn is_event_stream(headers: &reqwest::header::HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|ct| ct.split(';').next())
        .is_some_and(|essence| essence.trim().eq_ignore_ascii_case("text/event-stream"))
}

/// The ids of the requests `msg` carries (empty for notifications and
/// notification-only batches).
fn request_ids(msg: &Message) -> Vec<crate::types::RequestId> {
    match msg {
        Message::Request(r) => vec![r.id()],
        Message::Batch(batch) => batch
            .iter()
            .filter_map(|envelope| match envelope {
                crate::types::MessageEnvelope::Request(r) => Some(r.id()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

async fn start_sse_connection(
    session: Arc<McpSession>,
    resp_tx: mpsc::Sender<Result<Message, Error>>,
    auth: ClientAuth,
    #[cfg(feature = "client-tls")] tls_config: Option<ClientTlsConfig>,
) {
    let token = session.cancellation_token();
    tokio::select! {
        biased;
        _ = token.cancelled() => (),
        _ = session.initialized() => {
            tokio::spawn(handle_sse_connection(
                session.clone(),
                resp_tx,
                auth,
                #[cfg(feature = "client-tls")]
                tls_config
            ));
        }
    }
}

async fn handle_sse_connection(
    session: Arc<McpSession>,
    resp_tx: mpsc::Sender<Result<Message, Error>>,
    auth: ClientAuth,
    #[cfg(feature = "client-tls")] tls_config: Option<ClientTlsConfig>,
) {
    #[cfg(not(feature = "client-tls"))]
    let client = match create_client() {
        Ok(client) => client,
        Err(_err) => {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "SSE client error: {_err:#}");
            return;
        }
    };

    #[cfg(feature = "client-tls")]
    let client = match create_client(tls_config) {
        Ok(client) => client,
        Err(_err) => {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "SSE client error: {_err:#}");
            return;
        }
    };

    let token = session.cancellation_token();
    // At most one interactive re-authorization per (re)connection attempt
    // sequence -- a second consecutive 401 means the fresh token is not
    // accepted and the session must fail rather than loop.
    #[cfg(feature = "client-oauth")]
    let mut reauthorized = false;
    loop {
        let bearer = auth.fresh_bearer().await;
        let mut req = client
            .get(session.url())
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CACHE_CONTROL, "no-cache");

        if let Some(ref bearer) = bearer {
            req = req.bearer_auth(bearer);
        }

        if let Some(session_id) = session.session_id() {
            req = req.header(MCP_SESSION_ID, session_id.to_string());
        }

        if let Some(last_id) = session.last_event_id() {
            req = req.header(LAST_EVENT_ID, last_id);
        }

        let resp = match req.send().await {
            Ok(resp) => resp,
            Err(_err) => {
                #[cfg(feature = "tracing")]
                tracing::error!(logger = "neva", "Failed to send SSE request: {}", _err);
                session.cancellation_token().cancel();
                return;
            }
        };

        // A 401 under a managed OAuth session re-runs the authorization
        // flow once and retries the subscription with the fresh token.
        #[cfg(feature = "client-oauth")]
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED
            && !reauthorized
            && let ClientAuth::OAuth(oauth) = &auth
        {
            let challenge = resp
                .headers()
                .get(reqwest::header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            match oauth
                .authorize(challenge.as_deref(), bearer.as_deref())
                .await
            {
                Ok(_) => {
                    reauthorized = true;
                    continue;
                }
                Err(_err) => {
                    #[cfg(feature = "tracing")]
                    tracing::error!(logger = "neva", "OAuth authorization failed: {}", _err);
                }
            }
        }

        if !resp.status().is_success() {
            #[cfg(feature = "tracing")]
            tracing::error!(
                logger = "neva",
                "SSE request failed with status: {}",
                resp.status()
            );
            // Cancel the session so any in-flight init POST waiting on sse_ready()
            // is unblocked instead of hanging forever.
            session.cancellation_token().cancel();
            return;
        }

        #[cfg(feature = "client-oauth")]
        {
            reauthorized = false;
        }

        let mut stream = sse_stream::SseStream::from_bytes_stream(resp.bytes_stream())
            .fuse()
            .map_ok(|event| handle_event(event, &session, &resp_tx))
            .map_err(handle_error);

        session.notify_sse_initialized();

        loop {
            tokio::select! {
                biased;
                _ = token.cancelled() => return,
                fut = stream.next() => {
                    let Some(Ok(fut)) = fut else {
                        #[cfg(feature = "tracing")]
                        tracing::info!(logger = "neva", "SSE stream ended, reconnecting");
                        break;
                    };
                    fut.await;
                }
            }
        }

        // Stream ended -- wait before reconnecting to avoid hammering the server
        tokio::select! {
            biased;
            _ = token.cancelled() => return,
            _ = tokio::time::sleep(SSE_RECONNECT_DELAY) => {}
        }
    }
}

/// Drains a request-scoped SSE `POST` reply, forwarding every JSON-RPC message
/// it carries to the receive loop.
///
/// Returns whether the terminal reply arrived, so the caller can fail the
/// originating request when the stream ends without one.
async fn drain_post_sse<S>(mut stream: S, resp_tx: &mpsc::Sender<Result<Message, Error>>) -> bool
where
    S: futures_util::Stream<Item = Result<sse_stream::Sse, sse_stream::Error>> + Unpin,
{
    let mut answered = false;
    while let Some(event) = stream.next().await {
        match event {
            Ok(sse) if is_message_event(&sse) => {
                if forward_sse_message(sse, resp_tx).await {
                    answered = true;
                }
            }
            Ok(_) => {}
            Err(_err) => {
                #[cfg(feature = "tracing")]
                tracing::error!(logger = "neva", "SSE POST stream error: {}", _err);
                break;
            }
        }
    }
    answered
}

/// Whether an SSE frame carries a JSON-RPC message.
///
/// `message` is the *default* SSE event type, so a frame that omits `event:` and
/// one that names it explicitly mean the same thing. `Sse::is_message` only
/// covers the former (it is `event.is_none()`), so a peer that spells the type
/// out would otherwise have every frame -- notifications and the terminal
/// response alike -- discarded.
fn is_message_event(event: &sse_stream::Sse) -> bool {
    match &event.event {
        None => true,
        Some(kind) => kind.trim() == "message",
    }
}

async fn handle_event(
    event: sse_stream::Sse,
    session: &Arc<McpSession>,
    resp_tx: &mpsc::Sender<Result<Message, Error>>,
) {
    let id = event.id.clone();
    let delivered = if is_message_event(&event) {
        handle_msg(event, resp_tx).await
    } else {
        #[cfg(feature = "tracing")]
        tracing::debug!(logger = "neva", event = ?event);
        true
    };
    // Only advance the last event ID once the message is confirmed delivered,
    // so a reconnection will not skip events that were received but not processed.
    if delivered && let Some(id) = id {
        session.set_last_event_id(id);
    }
}

#[inline]
fn handle_error(_err: sse_stream::Error) {
    #[cfg(feature = "tracing")]
    tracing::error!(logger = "neva", "SSE Error: {}", _err);
}

// Returns true if the message was successfully parsed and delivered.
async fn handle_msg(
    event: sse_stream::Sse,
    resp_tx: &mpsc::Sender<Result<Message, Error>>,
) -> bool {
    let Some(data) = event.data else {
        return false;
    };
    // A malformed SSE frame must not reach the receive loop as a bare
    // `Err` (that would terminate it) -- log and skip; the last event id
    // does not advance, so a reconnect replays the event.
    let msg = match serde_json::from_str::<Message>(&data) {
        Ok(msg) => msg,
        Err(_err) => {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "Failed to parse SSE event: {}", _err);
            return false;
        }
    };
    if let Err(_err) = resp_tx.send(Ok(msg)).await {
        #[cfg(feature = "tracing")]
        tracing::error!(logger = "neva", "Failed to send server request: {}", _err);
        return false;
    }
    true
}

/// Forwards one frame of a request-scoped SSE `POST` reply to the receive loop.
///
/// Returns `true` only for the *terminal* reply -- a response or a batch
/// response -- so the caller can tell an orderly stream end from a truncated one
/// (notifications, and frames that fail to parse, return `false`).
async fn forward_sse_message(
    event: sse_stream::Sse,
    resp_tx: &mpsc::Sender<Result<Message, Error>>,
) -> bool {
    let Some(data) = event.data else {
        return false;
    };
    let msg = match serde_json::from_str::<Message>(&data) {
        Ok(msg) => msg,
        Err(_err) => {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "Failed to parse SSE POST event: {}", _err);
            return false;
        }
    };
    let terminal = matches!(msg, Message::Response(_) | Message::Batch(_));
    if let Err(_err) = resp_tx.send(Ok(msg)).await {
        #[cfg(feature = "tracing")]
        tracing::error!(logger = "neva", "Failed to send response: {}", _err);
        return false;
    }
    terminal
}

#[inline]
#[cfg(not(feature = "client-tls"))]
fn create_client() -> Result<reqwest::Client, Error> {
    reqwest::Client::builder().build().map_err(Error::from)
}

#[inline]
#[cfg(feature = "client-tls")]
fn create_client(mut tls_config: Option<ClientTlsConfig>) -> Result<reqwest::Client, Error> {
    let mut builder = reqwest::ClientBuilder::new();
    if let Some(ca_cert) = tls_config.as_mut().and_then(|tls| tls.ca.take()) {
        builder = builder.add_root_certificate(ca_cert);
    }
    if let Some(identity) = tls_config.as_mut().and_then(|tls| tls.identity.take()) {
        builder = builder.identity(identity);
    }
    if tls_config.is_some_and(|tls| !tls.certs_verification) {
        builder = builder.danger_accept_invalid_certs(true);
    }
    builder.build().map_err(Error::from)
}

impl From<reqwest::Error> for Error {
    #[inline]
    fn from(err: reqwest::Error) -> Self {
        Error::new(ErrorCode::ParseError, err.to_string())
    }
}

// These tests exercise the SSE GET stream path, which serves legacy
// peers -- compiled under both flags for the dual-mode client.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::http::ServiceUrl;

    fn make_session() -> Arc<McpSession> {
        Arc::new(McpSession::new(
            ServiceUrl::default(),
            CancellationToken::new(),
            #[cfg(not(feature = "legacy-spec"))]
            Default::default(),
        ))
    }

    // A minimal valid JSON-RPC notification that Message will accept
    const VALID_MSG: &str = r#"{"jsonrpc":"2.0","method":"ping"}"#;

    /// Only statuses that actually suggest an unknown method/route may
    /// yield `ParseError` -- the dual-mode fallback trigger. Upstream
    /// failures (auth, rate limit, 5xx) must stay `InternalError` so a
    /// valid 2026-07-28 peer is never mistaken for a legacy one.
    #[test]
    fn parse_failure_classifies_statuses() {
        let cases = [
            // legacy evidence: the peer answered, the body just isn't JSON-RPC
            (200, ErrorCode::ParseError),
            (202, ErrorCode::ParseError),
            (400, ErrorCode::ParseError),
            (404, ErrorCode::ParseError),
            (405, ErrorCode::ParseError),
            (406, ErrorCode::ParseError),
            // upstream failures: say nothing about the protocol generation
            (401, ErrorCode::InternalError),
            (403, ErrorCode::InternalError),
            (407, ErrorCode::InternalError),
            (429, ErrorCode::InternalError),
            (500, ErrorCode::InternalError),
            (502, ErrorCode::InternalError),
            (503, ErrorCode::InternalError),
            (504, ErrorCode::InternalError),
        ];

        for (status, expected) in cases {
            let status = reqwest::StatusCode::from_u16(status).unwrap();
            let (code, reason) = parse_failure(status, &"boom");
            assert_eq!(code, expected, "wrong code for HTTP {status}");
            assert!(
                reason.contains(status.as_str()),
                "the status must be carried in the message, got: {reason}"
            );
        }
    }

    #[tokio::test]
    async fn it_advances_last_event_id_on_successful_delivery() {
        let session = make_session();
        let (tx, mut rx) = mpsc::channel(1);

        let event = sse_stream::Sse::default().id("evt-1").data(VALID_MSG);
        handle_event(event, &session, &tx).await;

        assert_eq!(session.last_event_id(), Some("evt-1".to_string()));
        assert!(rx.try_recv().is_ok(), "message should have been delivered");
    }

    #[tokio::test]
    async fn it_does_not_advance_last_event_id_on_parse_failure() {
        let session = make_session();
        let (tx, _rx) = mpsc::channel(1);

        let event = sse_stream::Sse::default()
            .id("evt-bad")
            .data("not { valid json");
        handle_event(event, &session, &tx).await;

        assert!(session.last_event_id().is_none());
    }

    #[tokio::test]
    async fn it_does_not_advance_last_event_id_when_channel_closed() {
        let session = make_session();
        let (tx, rx) = mpsc::channel(1);
        drop(rx);

        let event = sse_stream::Sse::default().id("evt-dropped").data(VALID_MSG);
        handle_event(event, &session, &tx).await;

        assert!(session.last_event_id().is_none());
    }

    #[tokio::test]
    async fn it_advances_last_event_id_for_non_message_event() {
        let session = make_session();
        let (tx, _rx) = mpsc::channel(1);

        // Non-message SSE event (has event: field) -- no data sent to channel, but
        // ID should still advance so the server does not replay it on reconnect.
        let event = sse_stream::Sse::default()
            .id("evt-keepalive")
            .event("keepalive");
        handle_event(event, &session, &tx).await;

        assert_eq!(session.last_event_id(), Some("evt-keepalive".to_string()));
    }

    #[tokio::test]
    async fn it_does_not_advance_last_event_id_when_data_is_absent() {
        let session = make_session();
        let (tx, _rx) = mpsc::channel(1);

        // A message frame (no event: field) but data is None
        let event = sse_stream::Sse::default().id("evt-empty");
        handle_event(event, &session, &tx).await;

        assert!(session.last_event_id().is_none());
    }

    /// `message` is the default SSE event type, so naming it explicitly must be
    /// treated exactly like omitting it.
    #[test]
    fn explicitly_named_message_events_count_as_messages() {
        let cases = [
            (None, true),
            (Some("message"), true),
            (Some(" message "), true),
            (Some("Message"), false),
            (Some("keepalive"), false),
            (Some("endpoint"), false),
        ];

        for (kind, expected) in cases {
            let event = match kind {
                Some(kind) => sse_stream::Sse::default().event(kind),
                None => sse_stream::Sse::default(),
            };
            assert_eq!(
                is_message_event(&event),
                expected,
                "wrong verdict for event type {kind:?}"
            );
        }
    }

    /// A peer that frames its stream with `event: message` must have its payload
    /// delivered on both SSE paths -- the standalone `GET` and the POST reply.
    #[tokio::test]
    async fn named_message_events_are_delivered_on_both_sse_paths() {
        let response = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;

        // Standalone GET stream: delivered, and the last event id advances.
        let session = make_session();
        let (tx, mut rx) = mpsc::channel(1);
        let event = sse_stream::Sse::default()
            .id("evt-named")
            .event("message")
            .data(response);
        handle_event(event, &session, &tx).await;
        assert!(rx.try_recv().is_ok(), "GET frame must be delivered");
        assert_eq!(session.last_event_id(), Some("evt-named".to_string()));

        // Request-scoped POST reply, driven through the real drain loop: the
        // notification and the response are both delivered, and the stream counts
        // as answered so the request is not failed as truncated.
        let (tx, mut rx) = mpsc::channel(4);
        let frames = vec![
            Ok(sse_stream::Sse::default()
                .event("message")
                .data(r#"{"jsonrpc":"2.0","method":"notifications/message"}"#)),
            Ok(sse_stream::Sse::default().event("message").data(response)),
        ];
        assert!(
            drain_post_sse(futures_util::stream::iter(frames), &tx).await,
            "the POST stream must count as answered"
        );
        assert!(matches!(rx.try_recv(), Ok(Ok(Message::Notification(_)))));
        assert!(matches!(rx.try_recv(), Ok(Ok(Message::Response(_)))));
    }

    /// Frames of some other event type are skipped without answering the
    /// request, and a stream that carries only those ends unanswered.
    #[tokio::test]
    async fn drain_post_sse_skips_other_event_types() {
        let (tx, mut rx) = mpsc::channel(2);
        let frames = vec![
            Ok(sse_stream::Sse::default().event("keepalive").data("{}")),
            Ok(sse_stream::Sse::default()
                .event("endpoint")
                .data(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#)),
        ];
        assert!(!drain_post_sse(futures_util::stream::iter(frames), &tx).await);
        assert!(rx.try_recv().is_err(), "no frame should be delivered");
    }

    /// A request-scoped SSE `POST` reply must be recognized as *answered* only
    /// once its terminal reply arrives -- that flag is what tells a truncated
    /// stream (which has to fail the pending request) from an orderly one.
    #[tokio::test]
    async fn forward_sse_message_flags_only_terminal_replies() {
        let cases = [
            // (frame, is_terminal)
            (
                r#"{"jsonrpc":"2.0","method":"notifications/message"}"#,
                false,
            ),
            (r#"{"jsonrpc":"2.0","id":1,"result":{}}"#, true),
            (r#"[{"jsonrpc":"2.0","id":1,"result":{}}]"#, true),
        ];

        for (frame, terminal) in cases {
            let (tx, mut rx) = mpsc::channel(1);
            let event = sse_stream::Sse::default().data(frame);
            assert_eq!(
                forward_sse_message(event, &tx).await,
                terminal,
                "wrong terminal flag for {frame}"
            );
            assert!(rx.try_recv().is_ok(), "{frame} should still be delivered");
        }
    }

    #[tokio::test]
    async fn forward_sse_message_reports_unparseable_frame_as_unanswered() {
        let (tx, mut rx) = mpsc::channel(1);
        let event = sse_stream::Sse::default().data("not json");
        assert!(!forward_sse_message(event, &tx).await);
        assert!(
            rx.try_recv().is_err(),
            "a malformed frame must not reach the receive loop"
        );
    }

    /// Media types are case-insensitive and may carry parameters: mistaking such
    /// a reply for JSON would fail the request on an SSE-framed body.
    #[test]
    fn event_stream_media_type_is_matched_case_insensitively() {
        let cases = [
            ("text/event-stream", true),
            ("Text/Event-Stream", true),
            ("TEXT/EVENT-STREAM; charset=utf-8", true),
            ("text/event-stream ;charset=utf-8", true),
            ("application/json", false),
            ("text/event-streaming", false),
            ("application/json, text/event-stream", false),
        ];

        for (value, expected) in cases {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(CONTENT_TYPE, value.parse().unwrap());
            assert_eq!(
                is_event_stream(&headers),
                expected,
                "wrong verdict for content-type {value:?}"
            );
        }

        assert!(
            !is_event_stream(&reqwest::header::HeaderMap::new()),
            "a reply without content-type is not an SSE stream"
        );
    }
}

#[cfg(test)]
#[cfg(not(feature = "legacy-spec"))]
mod routing_hints_tests {
    use super::{name_param, routing_hints};
    use crate::transport::http::encode_header_value;
    use crate::types::notification::Notification;
    use crate::types::{Message, Request, RequestId};
    use serde_json::json;

    #[test]
    fn request_yields_method_and_no_name() {
        let req = Request::new::<()>(Some(RequestId::Number(1)), "tools/list", None);
        let msg = Message::Request(req);
        let hints = routing_hints(&msg).unwrap();
        assert_eq!(hints.0, "tools/list");
        assert!(hints.1.is_none());
    }

    #[test]
    fn tools_call_yields_method_and_tool_name() {
        let req = Request::new(
            Some(RequestId::Number(1)),
            "tools/call",
            Some(json!({"name": "echo", "arguments": {}})),
        );
        let msg = Message::Request(req);
        let hints = routing_hints(&msg).unwrap();
        assert_eq!(hints.0, "tools/call");
        assert_eq!(hints.1.as_deref(), Some("echo"));
    }

    /// The spec requires `Mcp-Name` on `tools/call`, `resources/read` and
    /// `prompts/get`, sourced from `params.name` / `params.uri`.
    #[test]
    #[cfg(not(feature = "legacy-spec"))]
    fn name_header_is_sourced_per_method() {
        use crate::types::{Request, RequestId};
        use serde_json::json;

        let cases = [
            ("tools/call", json!({ "name": "echo" }), Some("echo")),
            (
                "prompts/get",
                json!({ "name": "greeting" }),
                Some("greeting"),
            ),
            (
                "resources/read",
                json!({ "uri": "file:///a.txt" }),
                Some("file:///a.txt"),
            ),
            ("tools/list", json!({}), None),
        ];

        for (method, params, expected) in cases {
            let req = Request::new(Some(RequestId::Number(1)), method, Some(params));
            assert_eq!(name_param(&req).as_deref(), expected, "method: {method}");
        }
    }

    #[test]
    #[cfg(not(feature = "legacy-spec"))]
    fn header_values_are_encoded_when_not_ascii_safe() {
        // Plain ASCII passes through...
        assert_eq!(encode_header_value("us-west1"), "us-west1");
        // ...anything else travels Base64 behind the sentinel.
        assert_eq!(encode_header_value("caf\u{e9}"), "=?base64?Y2Fmw6k=?=");
        assert_eq!(encode_header_value(" lead"), "=?base64?IGxlYWQ=?=");
        assert_eq!(encode_header_value("trail "), "=?base64?dHJhaWwg?=");
        assert_eq!(encode_header_value("a\nb"), "=?base64?YQpi?=");
        // A plain value that looks like the sentinel must be encoded too, or a
        // server would decode something the client never encoded.
        assert_eq!(
            encode_header_value("=?base64?zzz?="),
            "=?base64?PT9iYXNlNjQ/enp6Pz0=?="
        );
    }

    #[test]
    fn notification_yields_method_only() {
        let n = Notification::new("notifications/cancelled", None);
        let msg = Message::Notification(n);
        let hints = routing_hints(&msg).unwrap();
        assert_eq!(hints.0, "notifications/cancelled");
        assert!(hints.1.is_none());
    }
}

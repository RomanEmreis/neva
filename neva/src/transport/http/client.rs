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
    /// Managed OAuth session — the token changes as flows complete.
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

// SSE constants — the standalone GET stream serves legacy peers only;
// its machinery compiles under both flags for the dual-mode client and
// activates at runtime when a legacy `initialize` handshake happens.
const LAST_EVENT_ID: HeaderName = HeaderName::from_static("last-event-id");
const SSE_RECONNECT_DELAY: Duration = Duration::from_secs(3);

#[cfg(feature = "proto-2026-07-28-rc")]
fn routing_hints(msg: &Message) -> Option<(&str, Option<&str>)> {
    match msg {
        Message::Request(r) => Some((r.method.as_str(), name_param(r))),
        Message::Notification(n) => Some((n.method.as_str(), None)),
        Message::Batch(_) | Message::Response(_) => None,
    }
}

#[cfg(feature = "proto-2026-07-28-rc")]
fn name_param(req: &crate::types::Request) -> Option<&str> {
    if req.method != crate::types::tool::commands::CALL {
        return None;
    }
    req.params.as_ref()?.as_object()?.get("name")?.as_str()
}

pub(super) async fn connect(rt: ClientRuntimeContext, token: CancellationToken) {
    let session = Arc::new(McpSession::new(
        rt.url,
        token,
        #[cfg(feature = "proto-2026-07-28-rc")]
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
    // `initialize` method) — against an RC peer it stays parked until
    // cancellation, so the stateless RC transport still issues only POSTs.
    tokio::join!(
        handle_connection(
            session.clone(),
            rt.rx,
            rt.tx.clone(),
            auth.clone(),
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
                    auth.clone()
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
) -> RequestBuilder {
    let mut resp = client
        .post(session.url())
        .json(req)
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream");

    if let Some(session_id) = session.session_id() {
        resp = resp.header(MCP_SESSION_ID, session_id.to_string())
    }

    // RC-peer routing headers: legacy servers never negotiated them, so
    // a peer that fell back to `initialize` gets the same wire shape a
    // pure legacy client produces (no routing headers, no RC protocol
    // version). Routing headers are exercised end-to-end via the
    // trace-context integration; unit-level hint extraction is tested in
    // `routing_hints_tests`.
    #[cfg(feature = "proto-2026-07-28-rc")]
    if !session.is_legacy() {
        if let Some((method, name)) = routing_hints(req) {
            resp = resp.header(crate::transport::http::MCP_METHOD, method);
            if let Some(n) = name {
                resp = resp.header(crate::transport::http::MCP_NAME, n);
            }
        }
        resp = resp.header(
            crate::transport::http::MCP_PROTOCOL_VERSION,
            crate::RC_PROTOCOL_VERSION,
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
) {
    let bearer = auth.fresh_bearer().await;
    let resp = match build_post(&client, &session, &req, bearer.as_deref())
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
                    match build_post(&client, &session, &req, Some(&fresh))
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

    match resp.json::<Message>().await {
        Ok(msg) => {
            if let Err(_err) = resp_tx.send(Ok(msg)).await {
                #[cfg(feature = "tracing")]
                tracing::error!(logger = "neva", "Failed to send response: {}", _err);
            }
        }
        // A reply that is not JSON-RPC — an HTML error page, or an error
        // code outside neva's `ErrorCode` set (e.g. the TS SDK's -32000).
        // Complete every originating request with an id-bound `ParseError`
        // response: a bare `Err` pushed into the channel would terminate
        // the receive loop without ever resolving the pending request.
        // This is also what lets `server/discover` classify such replies
        // and fall back to `initialize`.
        Err(err) => {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "Failed to parse HTTP response: {}", err);
            let reason = err.to_string();
            for id in request_ids(&req) {
                let resp = crate::types::Response::error(
                    id,
                    Error::new(ErrorCode::ParseError, reason.clone()),
                );
                if resp_tx.send(Ok(Message::Response(resp))).await.is_err() {
                    break;
                }
            }
        }
    }
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
    // sequence — a second consecutive 401 means the fresh token is not
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

        // Stream ended — wait before reconnecting to avoid hammering the server
        tokio::select! {
            biased;
            _ = token.cancelled() => return,
            _ = tokio::time::sleep(SSE_RECONNECT_DELAY) => {}
        }
    }
}

async fn handle_event(
    event: sse_stream::Sse,
    session: &Arc<McpSession>,
    resp_tx: &mpsc::Sender<Result<Message, Error>>,
) {
    let id = event.id.clone();
    let delivered = if event.is_message() {
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
    // `Err` (that would terminate it) — log and skip; the last event id
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
// peers — compiled under both flags for the dual-mode client.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::http::ServiceUrl;

    fn make_session() -> Arc<McpSession> {
        Arc::new(McpSession::new(
            ServiceUrl::default(),
            CancellationToken::new(),
            #[cfg(feature = "proto-2026-07-28-rc")]
            Default::default(),
        ))
    }

    // A minimal valid JSON-RPC notification that Message will accept
    const VALID_MSG: &str = r#"{"jsonrpc":"2.0","method":"ping"}"#;

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

        // Non-message SSE event (has event: field) — no data sent to channel, but
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

        // is_message() returns true (no event: field) but data is None
        let event = sse_stream::Sse::default().id("evt-empty");
        handle_event(event, &session, &tx).await;

        assert!(session.last_event_id().is_none());
    }
}

#[cfg(test)]
#[cfg(feature = "proto-2026-07-28-rc")]
mod routing_hints_tests {
    use super::routing_hints;
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
        assert_eq!(hints.1, Some("echo"));
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

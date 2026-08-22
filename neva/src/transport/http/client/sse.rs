//! The Server-Sent Events half of the transport.
//!
//! Two streams, with different lifetimes. The legacy `GET` stream is opened
//! once the `initialize` handshake completes and reconnects on its own,
//! resuming from `Last-Event-ID` so the server can replay what was missed; a
//! `POST` reply may itself be SSE-framed, and that stream lives only until the
//! response it carries arrives. Under MCP 2026-07-28 only the second exists.

use super::*;

pub(super) async fn start_sse_connection(
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

pub(super) async fn handle_sse_connection(
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
    // Whether this session has ever had the standalone stream open. It is what
    // tells the two meanings of a `404` on this verb apart; see below.
    let mut streamed = false;

    loop {
        let credential = auth.fresh_credential().await;
        let get = || {
            let mut req = client
                .get(session.url())
                .header(ACCEPT, "application/json, text/event-stream")
                .header(CACHE_CONTROL, "no-cache");

            if let Some(session_id) = session.session_id() {
                req = req.header(MCP_SESSION_ID, session_id.to_string());
            }

            if let Some(last_id) = session.last_event_id() {
                req = req.header(LAST_EVENT_ID, last_id);
            }

            req
        };

        let sent = send_authorized(
            credential.as_ref(),
            &reqwest::Method::GET,
            session.url(),
            get,
        )
        .await;

        let resp = match sent {
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
        //
        // So does a `403` whose challenge says `insufficient_scope`, on the
        // same reasoning the `POST` path uses: the token is valid and simply
        // does not cover this, which is the one `403` a wider grant fixes. A
        // server that guards its session stream with a scope its `POST`s do not
        // need would otherwise be unusable -- the client would never ask for
        // that scope, and the stream would die with the session.
        #[cfg(feature = "client-oauth")]
        if (resp.status() == reqwest::StatusCode::UNAUTHORIZED
            || (resp.status() == reqwest::StatusCode::FORBIDDEN
                && insufficient_scope(resp.headers())))
            && !reauthorized
            && let ClientAuth::OAuth(oauth) = &auth
        {
            let challenge = bearer_challenge(resp.headers());
            match oauth
                .authorize(
                    challenge.as_deref(),
                    credential.as_ref().map(Credential::access_token),
                )
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

        // A server that hosts no standalone stream says so with `405 Method Not
        // Allowed`, the status the spec names for exactly this. That is not a
        // failure: the GET stream is optional, and a client that reads "there
        // is no stream here" as a dead session refuses to talk to a conformant
        // server that simply chose not to offer one.
        //
        // The init POST is waiting on `sse_ready`, so it is released rather
        // than cancelled, and the session carries on over POST alone.
        //
        // `404` carries both meanings on this verb, and *when* it arrives is
        // what separates them. Before the stream has ever opened it is the
        // endpoint not routing `GET` at all -- servers answer a verb they do
        // not handle that way, the spec's `405` notwithstanding -- and the
        // handshake that just completed says the session is live. After a
        // stream that worked, the route plainly exists, so a `404` is the
        // session the request named being one the server no longer holds. That
        // one must not be swallowed: releasing the wait would leave the client
        // running on a session id every later POST is going to be refused for,
        // so it falls through to the cancellation below.
        if resp.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED
            || (resp.status() == reqwest::StatusCode::NOT_FOUND && !streamed)
        {
            #[cfg(feature = "tracing")]
            tracing::debug!(
                logger = "neva",
                "server offers no standalone SSE stream ({}); continuing over POST only",
                resp.status()
            );
            session.notify_sse_initialized();
            return;
        }

        if !resp.status().is_success() {
            #[cfg(feature = "tracing")]
            tracing::error!(
                logger = "neva",
                "SSE request failed with status: {}",
                resp.status()
            );
            // Any other non-2xx is about the session itself, not about the
            // stream being on offer -- a 401 says the credentials the POSTs
            // carry are wrong too. Cancel, so an in-flight init POST waiting on
            // `sse_ready()` fails with that rather than hanging forever.
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

        // The route exists, so from here a `404` can only be about the session.
        streamed = true;
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

        // Stream ended -- wait before reconnecting to avoid hammering the
        // server. How long is the server's call when it has stated one with an
        // SSE `retry:` field; the constant is only the answer for a server that
        // never said.
        tokio::select! {
            biased;
            _ = token.cancelled() => return,
            _ = tokio::time::sleep(session.retry_delay(SSE_RECONNECT_DELAY)) => {}
        }
    }
}

/// Drains a request-scoped SSE `POST` reply, forwarding every JSON-RPC message
/// it carries to the receive loop.
///
/// `ids` are the requests still owed an answer; [`Drained::owed`] is whatever
/// is *still* owed when the stream stops, so the caller can resume for exactly
/// those and fail exactly those.
///
/// Reading stops as soon as nothing is owed. That matters on the resumption
/// path: the stream replaying a truncated answer is the session's own `GET`,
/// which is long-lived and does not close once it has replayed. Draining it to
/// EOF would park this task on the session stream for the life of the client,
/// one leaked connection per truncated reply, competing with the standalone
/// `GET` for the traffic that follows.
///
/// The event id and the `retry:` delay are reported back rather than written to
/// the session. A legacy session runs two streams at once -- the standalone
/// `GET` and this request-scoped `POST` -- and each has its own position and its
/// own reconnection time. Sharing either lets a `GET` frame arriving between the
/// truncation and the resumption send this `POST` back to a place it never
/// reached, or reconnect it on a schedule the server named for the other stream;
/// and lets a `POST` frame do the same to the `GET`.
pub(super) async fn drain_post_sse<S>(
    mut stream: S,
    resp_tx: &mpsc::Sender<Result<Message, Error>>,
    ids: &[crate::types::RequestId],
) -> Drained
where
    S: futures_util::Stream<Item = Result<sse_stream::Sse, sse_stream::Error>> + Unpin,
{
    let mut owed = ids.to_vec();
    let mut last_event_id = None;
    let mut retry = None;
    while !owed.is_empty()
        && let Some(event) = stream.next().await
    {
        match event {
            Ok(sse) => {
                // Recorded before the frame is judged: a priming frame carries
                // no message, and is exactly where a server states the id to
                // resume from and how long to wait before doing so.
                //
                // Both stay with this stream. A reconnection time belongs to
                // the connection that was told it -- that is what the SSE
                // standard makes it -- and a legacy session runs two streams
                // whose lifetimes have nothing to do with each other: the
                // long-lived `GET` and this request-scoped reply. Writing this
                // one to the session would let whichever frame arrived last set
                // the other stream's delay, so a `retry: 0` here would have a
                // dropped `GET` reconnect instantly, and a patient `GET` would
                // hold up this resumption.
                if let Some(ms) = sse.retry {
                    retry = Some(ms);
                }

                if let Some(id) = sse.id.clone() {
                    last_event_id = Some(id);
                }

                if is_message_event(&sse) {
                    forward_sse_message(sse, resp_tx, &mut owed).await;
                }
            }
            Err(_err) => {
                #[cfg(feature = "tracing")]
                tracing::error!(logger = "neva", "SSE POST stream error: {}", _err);
                break;
            }
        }
    }
    Drained {
        owed,
        last_event_id,
        retry,
    }
}

/// What one pass over a request-scoped SSE stream left behind.
#[derive(Debug)]
pub(super) struct Drained {
    /// Requests this `POST` carried that are still unanswered.
    pub(super) owed: Vec<crate::types::RequestId>,
    /// The last `id:` this stream stated -- where a resumption of *this*
    /// stream picks up, which is not where the session's other stream is.
    pub(super) last_event_id: Option<String>,
    /// The `retry:` this stream stated, in milliseconds, if it stated one --
    /// how long before reopening *this* stream, and nobody else's.
    pub(super) retry: Option<u64>,
}

/// Whether this session can resume a dropped stream.
///
/// Resumption is a session-bound-transport affair: MCP 2026-07-28 removed both
/// the session and `Last-Event-ID`, so a 2026-07-28 peer has nothing to resume
/// against and a dropped stream there is simply a failed request.
pub(super) fn resumable(
    #[cfg_attr(feature = "legacy-spec", allow(unused_variables))] session: &McpSession,
) -> bool {
    #[cfg(not(feature = "legacy-spec"))]
    {
        session.is_legacy()
    }
    #[cfg(feature = "legacy-spec")]
    {
        true
    }
}

/// Reopens a dropped response stream and drains it for the answer it owed.
///
/// The server said when to come back (`retry:`) and where to resume from
/// (`id:`); both are honored, because reconnecting sooner hammers a server that
/// asked for room and reconnecting without the id makes it replay from the
/// start -- or from nothing. Both are what the dropped stream itself stated:
/// `retry` is `None` when it stated nothing, and the constant answers for it
/// rather than the standalone `GET`'s opinion of when to come back.
///
/// Returns what is still owed after this attempt.
pub(super) async fn resume_stream(
    client: &reqwest::Client,
    session: &McpSession,
    auth: &ClientAuth,
    last_event_id: &str,
    retry: Option<u64>,
    resp_tx: &mpsc::Sender<Result<Message, Error>>,
    ids: &[crate::types::RequestId],
) -> Vec<crate::types::RequestId> {
    let delay = retry.map_or(SSE_RECONNECT_DELAY, std::time::Duration::from_millis);
    let token = session.cancellation_token();
    tokio::select! {
        biased;
        _ = token.cancelled() => return ids.to_vec(),
        _ = tokio::time::sleep(delay) => {}
    }

    // Asked for here rather than carried from the `POST`, because the wait in
    // between is the server's to choose and may outlast the token that request
    // went out with. A managed session renews one that is about to expire
    // without troubling anybody.
    #[cfg_attr(not(feature = "client-oauth"), allow(unused_mut))]
    let mut credential = auth.fresh_credential().await;
    #[cfg(feature = "client-oauth")]
    let mut reauthorized = false;

    // Without the OAuth retry there is nothing to come back for, and the loop
    // is one pass by construction.
    #[cfg_attr(not(feature = "client-oauth"), allow(clippy::never_loop))]
    let resp = loop {
        let get = || {
            let mut req = client
                .get(session.url())
                .header(ACCEPT, "application/json, text/event-stream")
                .header(CACHE_CONTROL, "no-cache")
                .header(LAST_EVENT_ID, last_event_id);

            if let Some(session_id) = session.session_id() {
                req = req.header(MCP_SESSION_ID, session_id.to_string());
            }

            req
        };

        let sent = send_authorized(
            credential.as_ref(),
            &reqwest::Method::GET,
            session.url(),
            get,
        )
        .await;

        let resp = match sent {
            Ok(resp) => resp,
            Err(_err) => {
                #[cfg(feature = "tracing")]
                tracing::error!(logger = "neva", "Failed to resume SSE stream: {}", _err);
                return ids.to_vec();
            }
        };

        if resp.status().is_success() {
            break resp;
        }

        // The same authorization retry the `POST` and the standalone `GET` get,
        // for the same reason and once only. Treating a `401` here as final
        // throws away the answer this reconnection went back for -- the request
        // fails with an `InternalError` over a credential the client could have
        // renewed.
        #[cfg(feature = "client-oauth")]
        if !reauthorized
            && (resp.status() == reqwest::StatusCode::UNAUTHORIZED
                || (resp.status() == reqwest::StatusCode::FORBIDDEN
                    && insufficient_scope(resp.headers())))
            && let ClientAuth::OAuth(oauth) = auth
        {
            let challenge = bearer_challenge(resp.headers());
            if let Ok(fresh) = oauth
                .authorize(
                    challenge.as_deref(),
                    credential.as_ref().map(Credential::access_token),
                )
                .await
            {
                credential = Some(fresh);
                reauthorized = true;
                continue;
            }
        }

        #[cfg(feature = "tracing")]
        tracing::debug!(
            logger = "neva",
            "SSE resumption refused with status: {}",
            resp.status()
        );
        return ids.to_vec();
    };

    let stream = sse_stream::SseStream::from_bytes_stream(resp.bytes_stream());
    tokio::select! {
        biased;
        _ = token.cancelled() => ids.to_vec(),
        drained = drain_post_sse(stream, resp_tx, ids) => drained.owed,
    }
}

/// Whether an SSE frame carries a JSON-RPC message.
///
/// `message` is the *default* SSE event type, so a frame that omits `event:` and
/// one that names it explicitly mean the same thing. `Sse::is_message` only
/// covers the former (it is `event.is_none()`), so a peer that spells the type
/// out would otherwise have every frame -- notifications and the terminal
/// response alike -- discarded.
pub(super) fn is_message_event(event: &sse_stream::Sse) -> bool {
    match &event.event {
        None => true,
        Some(kind) => kind.trim() == "message",
    }
}

pub(super) async fn handle_event(
    event: sse_stream::Sse,
    session: &Arc<McpSession>,
    resp_tx: &mpsc::Sender<Result<Message, Error>>,
) {
    if let Some(retry) = event.retry {
        session.set_retry(retry);
    }
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
pub(super) fn handle_error(_err: sse_stream::Error) {
    #[cfg(feature = "tracing")]
    tracing::error!(logger = "neva", "SSE Error: {}", _err);
}

// Returns true if the message was successfully parsed and delivered.
pub(super) async fn handle_msg(
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
/// Strikes off `owed` every request this frame answers -- a response to one of
/// them, whether standalone or inside a batch -- so the caller can tell an
/// orderly stream end from a truncated one. Notifications, and frames that fail
/// to parse or to reach the receive loop, strike off nothing.
///
/// Both halves of that matter. A batch is not terminal by virtue of being a
/// batch: a subscription stream may deliver its acknowledgment and its events
/// batched. And a response is not terminal by virtue of being a response: one
/// carrying an id this `POST` never sent cannot resolve its pending slot. Either
/// mistake makes a stream that dies before the real response look orderly,
/// leaving a listen slot -- which carries no TTL -- with nothing to fail it and
/// `Subscription::closed` waiting on a result that is never coming.
///
/// A batch reply is struck off per response rather than wholesale: one frame
/// may answer some of what a batched `POST` asked and leave the rest to come.
pub(super) async fn forward_sse_message(
    event: sse_stream::Sse,
    resp_tx: &mpsc::Sender<Result<Message, Error>>,
    owed: &mut Vec<crate::types::RequestId>,
) {
    let Some(data) = event.data else {
        return;
    };

    let msg = match serde_json::from_str::<Message>(&data) {
        Ok(msg) => msg,
        Err(_err) => {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "Failed to parse SSE POST event: {}", _err);
            return;
        }
    };

    let answered: Vec<_> = match &msg {
        Message::Response(resp) => vec![resp.full_id()],
        Message::Batch(batch) => batch
            .iter()
            .filter_map(|env| match env {
                crate::types::MessageEnvelope::Response(resp) => Some(resp.full_id()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };

    // Struck off only once the message is on its way to the receive loop: a
    // frame that never gets there has resolved nothing, and the caller must
    // still fail the request rather than assume it was answered.
    if let Err(_err) = resp_tx.send(Ok(msg)).await {
        #[cfg(feature = "tracing")]
        tracing::error!(logger = "neva", "Failed to send response: {}", _err);
        return;
    }

    owed.retain(|id| !answered.contains(id));
}

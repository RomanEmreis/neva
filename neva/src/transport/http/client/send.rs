//! Building a POST and getting its answer back.
//!
//! [`exchange`] is where the transport's awkward cases live: a reply may be a
//! single JSON document or an SSE stream that ends with the response, a `401`
//! may mean "authorize and retry once", and a body that is not JSON-RPC at all
//! has to be classified carefully -- some of those statuses are what the
//! dual-mode client reads as "this peer is a generation behind".

use super::*;

/// Builds the JSON-RPC POST with all transport headers set.
///
/// The credential is not among them: a DPoP proof is signed over the request
/// it accompanies and cannot be reused across attempts, so attaching one is
/// [`send_authorized`]'s job and happens once per attempt.
pub(super) fn build_post(
    client: &reqwest::Client,
    session: &McpSession,
    req: &Message,
    #[cfg(not(feature = "legacy-spec"))] mirrored: &[(String, String)],
) -> RequestBuilder {
    // `.json()` already sets `Content-Type: application/json`, and `.header()`
    // *appends* rather than replaces -- setting it again put the header on the
    // wire twice. A receiver that reads the header as a list then sees
    // `"application/json, application/json"`, which matches no media type it
    // knows, and answers `415 Unsupported Media Type`.
    let mut resp = client
        .post(session.url())
        .json(req)
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

        for (name, value) in mirrored {
            resp = resp.header(
                name.as_str(),
                crate::transport::http::encode_header_value(value),
            );
        }

        resp = resp.header(
            crate::transport::http::MCP_PROTOCOL_VERSION,
            crate::LATEST_PROTOCOL_VERSION,
        );
    }

    resp
}

/// Sends one message, racing the whole exchange against a cancellation of the
/// subscription it opens (if it opens one).
///
/// The race wraps *everything* rather than individual awaits: a cancel can land
/// while the token is being refreshed, while an authorization flow runs, while
/// the peer sits on the response headers, or mid-stream. Dropping the inner
/// future at any of those points drops the request and its response body, which
/// is exactly the close the server reads as "this subscription is over".
///
/// `abort` is handed in already registered -- see [`track_listen`] for why the
/// registration cannot happen in here.
pub(super) async fn send_request(
    client: reqwest::Client,
    session: Arc<McpSession>,
    req: Message,
    resp_tx: mpsc::Sender<Result<Message, Error>>,
    auth: ClientAuth,
    #[cfg(not(feature = "legacy-spec"))] param_registry: crate::shared::param_headers::Registry,
    #[cfg(not(feature = "legacy-spec"))] abort: ListenAbort,
) {
    #[cfg(not(feature = "legacy-spec"))]
    if abort.is_tracked() {
        // The session token belongs in this race too: `Client::disconnect`
        // cancels it and the connection loop exits, but a listen POST is the
        // one request nothing else stops -- it would go on draining its body,
        // holding the server-side subscription open past the disconnect.
        let session_token = session.cancellation_token();
        tokio::select! {
            _ = exchange(client, session, req, resp_tx, auth, param_registry) => {}
            _ = abort.cancelled() => {}
            _ = session_token.cancelled() => {}
        }
        return;
    }

    exchange(
        client,
        session,
        req,
        resp_tx,
        auth,
        #[cfg(not(feature = "legacy-spec"))]
        param_registry,
    )
    .await
}

/// The exchange itself: send, handle a managed-OAuth retry, then read the reply
/// (a single body, or a stream drained into the receive loop).
pub(super) async fn exchange(
    client: reqwest::Client,
    session: Arc<McpSession>,
    req: Message,
    resp_tx: mpsc::Sender<Result<Message, Error>>,
    auth: ClientAuth,
    #[cfg(not(feature = "legacy-spec"))] param_registry: crate::shared::param_headers::Registry,
) {
    // Only this exchange's own requests use it. A resumption `GET` asks `auth`
    // again when its turn comes, so a flow completing in between -- here or
    // anywhere else -- reaches it without being threaded through.
    let credential = auth.fresh_credential().await;
    // Once for the whole exchange -- see `mirrored_param_headers`.
    #[cfg(not(feature = "legacy-spec"))]
    let mirrored = mirrored_param_headers(&session, &req, &param_registry);

    let post = || {
        build_post(
            &client,
            &session,
            &req,
            #[cfg(not(feature = "legacy-spec"))]
            &mirrored,
        )
    };

    let sent = send_authorized(
        credential.as_ref(),
        &reqwest::Method::POST,
        session.url(),
        post,
    )
    .await;

    let resp = match sent {
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
    //
    // A `403` counts when its challenge says `insufficient_scope`: the token is
    // valid and simply does not cover this call, which is the one 403 a fresh
    // authorization can fix. Any other 403 is a decision about the caller, not
    // about the token, and re-authorizing would only ask the user to approve
    // something that will be refused again.
    #[cfg(feature = "client-oauth")]
    let resp = match (&auth, resp.status()) {
        (ClientAuth::OAuth(oauth), status)
            if status == reqwest::StatusCode::UNAUTHORIZED
                || (status == reqwest::StatusCode::FORBIDDEN
                    && insufficient_scope(resp.headers())) =>
        {
            let challenge = bearer_challenge(resp.headers());
            match oauth
                .authorize(
                    challenge.as_deref(),
                    credential.as_ref().map(Credential::access_token),
                )
                .await
            {
                Ok(fresh) => {
                    let retried =
                        send_authorized(Some(&fresh), &reqwest::Method::POST, session.url(), post)
                            .await;

                    match retried {
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
        let ids = request_ids(&req);

        let Drained {
            mut owed,
            last_event_id,
            retry,
        } = drain_post_sse(stream, &resp_tx, &ids).await;

        // A stream that ended before the response is not necessarily a failed
        // request: on the session-bound transport the server may finish the
        // answer on a resumed stream, which is what event ids and the `retry:`
        // field are for. One attempt, and only when the server named an id to
        // resume from -- without one there is nothing to ask it to replay, and
        // more than one turns a server that keeps dropping the stream into a
        // reconnect loop the caller cannot see.
        //
        // Both the id and the delay are the ones *this* stream stated. The
        // session's other stream has its own position and its own idea of how
        // long to wait; borrowing either would ask the server to replay from
        // somewhere this request never was, or to be reconnected on a schedule
        // it never asked this stream for.
        //
        // The resumption asks for what is still owed rather than for everything
        // the `POST` carried: a batch whose stream died midway has some of its
        // answers already, and re-delivering those would resolve nothing.
        if !owed.is_empty()
            && resumable(&session)
            && let Some(last_id) = last_event_id
        {
            owed = resume_stream(&client, &session, &auth, &last_id, retry, &resp_tx, &owed).await;
        }

        // A truncated stream, an unparseable frame, or EOF before the final
        // response would otherwise leave the originating request sitting in the
        // pending queue until it times out. Fail it now, id-bound, exactly like
        // the non-JSON-RPC reply path below. `InternalError` (not `ParseError`)
        // on purpose: the peer clearly speaks 2026-07-28, so this must not be mistaken
        // for dual-mode fallback evidence.
        if !owed.is_empty() {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", STREAM_ENDED_BEFORE_RESPONSE);
            for id in owed {
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

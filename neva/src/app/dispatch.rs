//! The path a message takes from the transport to a handler and back.
//!
//! [`App::run`](super::App::run) hands each inbound message to
//! [`App::execute`], which drives it through the middleware pipeline;
//! `message_middleware` sits at the end of that pipeline, routes the message by
//! kind, awaits the handler and sends the response.
//!
//! Everything request-scoped that has to outlive the handler is set up and torn
//! down here: the tracing span, the notification sink, the in-flight count the
//! shutdown drain reads, and the MRTR round (whose own plumbing lives in
//! [`super::mrtr`]).

use super::*;

impl App {
    #[cfg(feature = "tracing")]
    pub(super) async fn tracing_middleware(ctx: MwContext, next: Next) -> Response {
        #[cfg(feature = "legacy-spec")]
        let span = create_tracing_span(ctx.session_id().cloned());
        // 2026-07-28: the request-scoped logging level rides on the originating
        // request's `_meta`. Stamp it onto the span so the notification layer
        // can decide which `notifications/message` to deliver for this request.
        #[cfg(not(feature = "legacy-spec"))]
        let span = {
            let log_level = ctx
                .request()
                .and_then(|req| req.meta())
                .and_then(|meta| meta.log_level);
            create_tracing_span(ctx.session_id().cloned(), log_level)
        };

        next(ctx).instrument(span).await
    }

    #[inline]
    pub(super) async fn execute(msg: Message, runtime: ServerRuntime) {
        // Held for the whole pipeline, so the shutdown drain can tell a
        // response that is queued from one that is still being produced.
        #[cfg(not(feature = "legacy-spec"))]
        let _in_flight = InFlightGuard::enter(runtime.in_flight());
        // Closing the request notification sink here -- once the *whole*
        // middleware pipeline has run -- is what lets a request-scoped SSE POST
        // response know no more notifications are coming, so logs emitted by
        // user middleware after `next(ctx)` still make it onto the stream.
        #[cfg(all(not(feature = "legacy-spec"), feature = "http-server"))]
        let _sink_guard = RequestSinkGuard(msg.session_id().copied());

        runtime.execute(msg).await;
    }

    pub(super) async fn execute_batch(batch: MessageBatch, runtime: ServerRuntime) {
        use crate::transport::TransportProtoSender;
        use futures_util::future::join_all;

        // Capture the incoming batch's correlation and HTTP-context fields.
        // `id` + `session_id` are needed so the response batch can be routed
        // back to the correct waiting HTTP handler.  `headers` and `claims`
        // are copied onto every inner Request so that middleware (auth checks,
        // role/permission guards, SSE routing) sees the original HTTP context.
        let batch_id = batch.id.clone();
        let batch_session_id = batch.session_id;
        // One guard for the whole batch (inner requests run through
        // `ServerRuntime::execute`, so they never close the shared sink early):
        // the request-scoped SSE response stays open until every inner request
        // and its middleware have finished. See `App::execute`.
        #[cfg(not(feature = "legacy-spec"))]
        let _in_flight = InFlightGuard::enter(runtime.in_flight());
        #[cfg(all(not(feature = "legacy-spec"), feature = "http-server"))]
        let _sink_guard = RequestSinkGuard(batch_session_id);
        #[cfg(feature = "http-server")]
        let batch_headers = batch.headers.clone();
        #[cfg(feature = "http-server")]
        let batch_claims = batch.claims.clone();

        let real_sender = runtime.sender();

        // Collect responses produced by batch request handlers in-memory.
        // Server-initiated messages (sampling, elicitation, notifications) go
        // straight to the real transport inside BatchCollect::send, so handlers
        // that call ctx.elicit()/ctx.sample() never deadlock.
        //
        // Crucially, background tasks that capture a BatchCollect sender clone
        // do NOT block the batch response: we only wait for the join_all futures,
        // then snapshot whatever responses have been collected so far.
        let responses: Arc<std::sync::Mutex<Vec<MessageEnvelope>>> = Arc::default();
        let batch_sender = TransportProtoSender::BatchCollect {
            real_sender: Arc::new(tokio::sync::Mutex::new(real_sender.clone())),
            responses: Arc::clone(&responses),
        };

        // Capture before consuming the batch so we know whether to send an ack
        // when all Response envelopes were consumed by pending.complete (section below).
        let has_error_responses = batch
            .iter()
            .any(|e| matches!(e, MessageEnvelope::Response(Response::Err(_))));

        let futures = batch.into_iter().map(|envelope| {
            let runtime = runtime.clone();
            let mut sender = batch_sender.clone();
            // Clone per-iteration so each async move block owns its own copy.
            #[cfg(feature = "http-server")]
            let batch_headers = batch_headers.clone();
            #[cfg(feature = "http-server")]
            let batch_claims = batch_claims.clone();

            async move {
                match envelope {
                    MessageEnvelope::Request(mut req) => {
                        // Copy the batch's HTTP metadata onto the inner request
                        // so that session/auth context is preserved: without
                        // this, role/permission checks can fail with a valid
                        // token and server-initiated follow-up calls (sampling,
                        // elicitation) cannot be routed back over SSE.
                        req.session_id = batch_session_id;
                        #[cfg(feature = "http-server")]
                        {
                            req.headers = batch_headers;
                            req.claims = batch_claims;
                        }
                        runtime
                            .with_sender(sender)
                            .execute(Message::Request(req))
                            .await;
                    }
                    MessageEnvelope::Notification(notification) => {
                        Self::handle_notification(notification, runtime.clone()).await;
                    }
                    MessageEnvelope::Response(mut resp) => {
                        // Apply the batch's session context so that
                        // `resp.full_id()` (= session_id + resp_id) matches
                        // the key used when the server registered the pending
                        // request via `send_request`. Without this the lookup
                        // in the pending queue misses and the pending handler leaks.
                        if let Some(session_id) = batch_session_id {
                            resp = resp.set_session_id(session_id);
                        }
                        #[cfg(feature = "http-server")]
                        {
                            resp = resp.set_headers(batch_headers);
                        }
                        // An unmatched error is the deserializer's synthetic
                        // `InvalidRequest` for a malformed batch item, so it goes
                        // to the collector to appear in the reply. An unmatched
                        // `Ok` is unsolicited or stale and is dropped, as on the
                        // single-message path.
                        if let Some(handle) = runtime.pending_requests().pop(&resp.full_id()) {
                            handle.send(resp);
                        } else if matches!(resp, Response::Err(_)) {
                            let _ = sender.send(Message::Response(resp)).await;
                        }
                    }
                }
            }
        });

        join_all(futures).await;

        // A response a background task produces after this point arrived too
        // late for the batch reply and is discarded.
        let envelopes = responses
            .lock()
            .map(|mut guard| std::mem::take(&mut *guard))
            .unwrap_or_default();

        if envelopes.is_empty() {
            if has_error_responses {
                // Every error was a real peer response, consumed by `pop` above,
                // so the batch produced nothing to reply with -- but the HTTP
                // transport opened a pending slot for it and would wait out its
                // timeout. The empty ack is what closes that slot.
                let mut ack = Response::empty(batch_id);
                if let Some(session_id) = batch_session_id {
                    ack = ack.set_session_id(session_id);
                }

                let mut sender = real_sender;
                let _ = sender.send(Message::Response(ack)).await;
            }
            return;
        }

        let mut resp_batch = match MessageBatch::new(envelopes) {
            Ok(b) => b,
            Err(_err) => {
                // Unreachable in practice: envelopes are non-empty above.
                #[cfg(feature = "tracing")]
                tracing::error!(
                    logger = "neva",
                    "Failed to construct batch response: {:?}",
                    _err
                );
                return;
            }
        };
        // Restore the correlation id+session so the HTTP transport can match
        // this response batch to the waiting HTTP handler.
        resp_batch.id = batch_id;
        resp_batch.session_id = batch_session_id;

        let mut sender = real_sender;
        if let Err(_err) = sender.send(Message::Batch(resp_batch)).await {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "Error sending batch response: {:?}", _err);
        }
    }

    pub(super) async fn message_middleware(ctx: MwContext, _: Next) -> Response {
        let MwContext {
            msg,
            runtime,
            #[cfg(feature = "di")]
            scope,
        } = ctx;

        let id = msg.id();
        let mut sender = runtime.sender();

        if let Some(resp) = Self::handle_message(
            msg,
            runtime,
            #[cfg(feature = "di")]
            scope,
        )
        .await
            && let Err(_err) = sender.send(resp.into()).await
        {
            #[cfg(feature = "tracing")]
            tracing::error!(
                logger = "neva",
                error = format!("Error sending response: {:?}", _err)
            );
        }

        Response::empty(id)
    }

    #[inline]
    async fn handle_message(
        msg: Message,
        runtime: ServerRuntime,
        #[cfg(feature = "di")] scope: Container,
    ) -> Option<Response> {
        match msg {
            Message::Request(req) => Some(
                Self::handle_request(
                    req,
                    runtime,
                    #[cfg(feature = "di")]
                    scope,
                )
                .await,
            ),
            Message::Response(resp) => Some(Self::handle_response(resp, runtime).await),
            Message::Notification(notification) => {
                // JSON-RPC 2.0 section 4: notifications must never receive a response.
                Self::handle_notification(notification, runtime).await;
                None
            }
            Message::Batch(_) => {
                // Batches are dispatched via execute_batch before reaching handle_message
                unreachable!(
                    "Message::Batch should be intercepted in App::run before handle_message"
                )
            }
        }
    }

    async fn handle_request(
        req: Request,
        runtime: ServerRuntime,
        #[cfg(feature = "di")] scope: Container,
    ) -> Response {
        #[cfg(feature = "http-server")]
        let mut req = req;
        let req_id = req.id();
        let session_id = req.session_id;
        let full_id = req.full_id();

        // MRTR pre-capture: method + salient params (the params that identify
        // this request, see `salient_params`), needed after `req`/`context` are
        // moved into `handler.call`.
        #[cfg(not(feature = "legacy-spec"))]
        let mrtr_method = shared::is_mrtr_method(&req.method);
        #[cfg(not(feature = "legacy-spec"))]
        let req_method = req.method.clone();
        #[cfg(not(feature = "legacy-spec"))]
        let salient_params = req
            .params
            .as_ref()
            .map(salient_params)
            .unwrap_or(serde_json::Value::Null);

        // What MCP 2026-07-28 requires of every request's `_meta`: the
        // mandatory fields, and a protocol version this build actually speaks.
        // The HTTP preamble rejects both earlier so it can attach the `400` the
        // spec asks for; this seam is what every other transport gets, since
        // the requirements are on the message and not on how it travelled.
        //
        // The MRTR field check rides along, but only for the methods MRTR runs
        // on. `requestState` and `inputResponses` are protocol fields *there*;
        // on a custom `map_handler` method they are just params, and a handler
        // is entitled to a numeric `requestState` of its own. Judging those by
        // the MRTR shapes would refuse a request this server was written to
        // serve.
        #[cfg(not(feature = "legacy-spec"))]
        if let Some(err) = req
            .required_meta_error()
            .or_else(|| req.unsupported_version_error())
            .or_else(|| mrtr_method.then(|| req.malformed_mrtr_error()).flatten())
        {
            let mut resp = Response::error(req_id, err);
            if let Some(session_id) = session_id {
                resp = resp.set_session_id(session_id);
            }
            return resp;
        }

        // The `Mcp-Param-*` half of header validation. The transport preamble
        // checks the standard routing headers, but these are defined by the
        // called tool's own `x-mcp-header` annotations -- which only this side
        // of the channel knows -- so the check lands where the tool registry
        // does. The resulting `-32020` picks up its mandated `400` from the
        // transport's status mapping on the way out.
        #[cfg(all(feature = "http-server", not(feature = "legacy-spec")))]
        if let Some(err) = param_header_error(&req, &runtime.options()).await {
            let mut resp = Response::error(req_id, err);
            // The transport correlates a reply by `session_id`+`id`; a reply
            // that leaves without one is never matched to the POST waiting for
            // it. Every other return path down this function does the same.
            if let Some(session_id) = session_id {
                resp = resp.set_session_id(session_id);
            }

            return resp;
        }

        #[cfg(not(feature = "http-server"))]
        let context = runtime.context(session_id);

        #[cfg(feature = "http-server")]
        let context = {
            let headers = std::mem::take(&mut req.headers);
            let claims = req.claims.take();
            runtime.context(session_id, headers, claims)
        };

        #[cfg(feature = "di")]
        let context = context.with_scope(scope);

        let options = runtime.options();
        let handlers = runtime.request_handlers();
        let token = options.track_request(&full_id);

        // MRTR seed: decode/verify any incoming `requestState`, merge this
        // round's `inputResponses`, and attach the replay state to the context.
        #[cfg(not(feature = "legacy-spec"))]
        let mut context = context;
        // Declared per request, so it belongs on every dispatch and not just the
        // MRTR ones: a task-augmented call asks the same caller for the same
        // kinds of input, and a handler that skips asking when the caller cannot
        // answer needs to know that on any substrate.
        #[cfg(not(feature = "legacy-spec"))]
        {
            context.client_capabilities = req
                .meta()
                .as_ref()
                .and_then(|m| m.client_capabilities)
                .unwrap_or_default();
        }
        #[cfg(not(feature = "legacy-spec"))]
        let (mrtr_arc, mrtr_principal) = if mrtr_method {
            #[cfg(feature = "http-server")]
            let principal = context
                .claims
                .as_ref()
                .and_then(|c| c.subject().map(|s| s.to_owned()));
            #[cfg(not(feature = "http-server"))]
            let principal: Option<String> = None;

            match seed_mrtr_ctx(
                &req,
                &req_method,
                &salient_params,
                &options,
                principal.as_deref(),
            ) {
                Ok(arc) => {
                    context.exec = crate::app::context::ExecMode::Mrtr(arc.clone());
                    (Some(arc), principal)
                }
                Err(e) => {
                    options.complete_request(&full_id);
                    let mut resp = Err::<Response, _>(e).into_response(req_id);
                    if let Some(session_id) = session_id {
                        resp = resp.set_session_id(session_id);
                    }
                    return resp;
                }
            }
        } else {
            (None, None)
        };

        // MRTR idempotency: the final-round response cache is keyed by the
        // incoming state's sealed segment (the ciphertext+tag after the last
        // `.` -- `rsplit_once` keeps grabbing it regardless of the leading
        // `v1.kid.` header segments -- unique per minted state thanks to the
        // random AEAD nonce) *plus* a digest of the answers this round actually
        // resolved to. The answers digest matters because the *same* minted
        // state can be echoed with *different* answers -- a client (or
        // attacker) replaying one round-1 blob with two different
        // `inputResponses` would otherwise hit the first answer's cached result
        // for the second. Folding in the answers' digest keeps those apart,
        // while a genuine lost-response retry -- same state *and* same answers
        // -- still hits. Only committed *final* rounds are ever cached, so a
        // hit here is by construction a replay of one.
        //
        // The digest is taken over the *seeded* answers rather than the raw
        // `inputResponses` off the wire, and the difference is the whole
        // protection. `seed_mrtr_ctx` drops an answer that is unsolicited or
        // already settled, so two requests can carry different `inputResponses`
        // and still resolve to identical input for the handler. Keying on the
        // raw map would give those two different keys: a replay that merely
        // adds a junk key, or re-answers a key the state already sealed, would
        // miss the cache and run the final handler -- and its `on_commit`
        // effects -- a second time. Keying on what the handler will actually
        // see makes the key a function of the work, which is what the cache is
        // there to deduplicate.
        #[cfg(not(feature = "legacy-spec"))]
        let state_tag: Option<String> = if mrtr_method {
            req.state().and_then(|state| {
                let (_, tag) = state.rsplit_once('.')?;
                let answers = mrtr_arc
                    .as_ref()
                    .map(|arc| crate::types::mrtr::state::input_responses_digest(&arc.answers))
                    .unwrap_or_default();
                Some(format!("{tag}.{answers}"))
            })
        } else {
            None
        };
        // Claim the per-state reservation *before* the cache lookup and hold it
        // through the handler, commits and the final `put`. Two identical
        // final-round retries (e.g. a client that timed out and re-sent while
        // the first round is still committing) would otherwise both miss the
        // cache below and re-run the handler + `on_commit` effects. The loser
        // blocks here until the winner has cached, then hits it instead.
        //
        // NOTE: this guard MUST stay live until after the final `put` below.
        // Dropping it early (e.g. rewriting to `let _ = ...reserve().await;`)
        // releases the lock immediately and reopens the concurrent-retry race;
        // the explicit, self-describing name guards against that refactor.
        #[cfg(not(feature = "legacy-spec"))]
        let _reservation_guard_held_through_commit = match state_tag.as_deref() {
            Some(tag) => Some(options.request_state_store().reserve(tag).await),
            None => None,
        };

        #[cfg(not(feature = "legacy-spec"))]
        if let Some(tag) = state_tag.as_deref()
            && let Some(cached) = options.request_state_store().get(tag).await
        {
            options.complete_request(&full_id);
            let mut resp = cached.set_id(req_id.clone());
            if let Some(session_id) = session_id {
                resp = resp.set_session_id(session_id);
            }

            return resp;
        }

        #[cfg(feature = "tracing")]
        tracing::trace!(logger = "neva", "Received: {:?}", req);
        let resp = if let Some(handler) = handlers.get(&req.method) {
            tokio::select! {
            resp = handler.call(HandlerParams::Request(context, req)) => {
                options.complete_request(&full_id);
                resp
            }
            _ = token.cancelled() => {
                #[cfg(feature = "tracing")]
                tracing::debug!(
                    logger = "neva",
                    "The request with ID: {} has been cancelled", full_id);
                    Err(Error::from(ErrorCode::RequestCancelled))
                }
            }
        } else {
            Err(Error::from(ErrorCode::MethodNotFound))
        };

        // MRTR interception: if the handler requested input (recorded in the
        // shared `MrtrCtx`), convert to an `InputRequiredResult` regardless of
        // what the handler returned. The pending flag -- not the sentinel error
        // -- is the reliable signal, because tool/prompt/resource wrappers fold
        // a handler `Err` into an in-band error result before we see it.
        #[cfg(not(feature = "legacy-spec"))]
        let mut cache_final = false;
        #[cfg(not(feature = "legacy-spec"))]
        let resp = match (mrtr_method, mrtr_arc) {
            (true, Some(arc)) => {
                // An answer that does not fit the kind it answers outranks
                // everything else this round, re-requesting included: asking
                // again for a key the client already answered wrongly is how a
                // chain loops forever. It is the client's protocol mistake, so
                // it is answered as one and not as a call that ran and failed.
                let malformed = arc
                    .malformed_answer
                    .lock()
                    .map(|mut m| m.take())
                    .unwrap_or_default();
                let has_pending = arc.pending.lock().map(|p| !p.is_empty()).unwrap_or(false);
                if let Some(reason) = malformed {
                    Err(Error::new(ErrorCode::InvalidParams, reason))
                } else if has_pending {
                    build_input_required(
                        &arc,
                        &req_method,
                        &salient_params,
                        &options,
                        mrtr_principal,
                    )
                    .map(|ir| ir.into_response(req_id.clone()))
                } else if mrtr_should_commit(&resp) {
                    // Final round: run deferred commits in registration order.
                    // The first Err becomes the response error.
                    let commits = arc
                        .commits
                        .lock()
                        .map(|mut c| std::mem::take(&mut *c))
                        .unwrap_or_default();
                    let mut commit_err = None;
                    for fut in commits {
                        if let Err(e) = fut.await {
                            commit_err = Some(e);
                            break;
                        }
                    }
                    // Cache this round's answer either way (below, once the id
                    // is final), so a lost-response retry is served from it
                    // rather than run again.
                    //
                    // Failure has to be cached too, and that is the whole point
                    // rather than an afterthought: by here the commits have
                    // *started*. An earlier one may have already applied its
                    // effect, and the one that returned `Err` may have applied
                    // part of its own. Leaving the round uncached would send an
                    // identical retry back through the handler and re-run those
                    // effects -- charging twice to report the same failure,
                    // which is exactly what `on_commit` exists to prevent.
                    //
                    // The cost is that the state carries its failure: a retry
                    // of this round replays the error instead of getting
                    // another attempt. That is the safe direction. Recovering
                    // means starting a fresh flow, whose new state re-runs
                    // everything deliberately -- the case `on_commit`'s docs
                    // already call out as outside its guarantee.
                    cache_final = true;
                    match commit_err {
                        Some(e) => Err(e),
                        None => resp,
                    }
                } else {
                    resp
                }
            }
            _ => resp,
        };

        let mut resp = resp.into_response(req_id);
        // The two things MCP 2026-07-28 puts on every result: the discriminator
        // and the server's own identity (`serverInfo` left `DiscoverResult`).
        // Both are stamped here, at the single seam every dispatched response
        // passes through -- a handler that returns a `Response` it built
        // elsewhere, or proxied from an upstream peer, reaches the wire without
        // passing `Response::success`.
        #[cfg(not(feature = "legacy-spec"))]
        {
            resp = resp
                .with_result_type()
                .with_server_info(&options.implementation);
        }

        if let Some(session_id) = session_id {
            resp = resp.set_session_id(session_id);
        }

        // Record the committed final response under the incoming state's tag
        // plus this round's answers digest (see `state_tag` above). Retained
        // until the state's own expiry window (`now + ttl`, an upper bound on
        // its remaining life), after which a retry is rejected as expired before
        // reaching the store.
        #[cfg(not(feature = "legacy-spec"))]
        if cache_final && let Some(tag) = state_tag.as_deref() {
            let exp = crate::types::mrtr::state::now_secs() + options.request_state_ttl_secs();
            options
                .request_state_store()
                .put(tag, resp.clone(), exp)
                .await;
        }
        resp
    }

    async fn handle_response(resp: Response, runtime: ServerRuntime) -> Response {
        let resp_id = resp.id().clone();
        let session_id = resp.session_id().cloned();

        // A suspended task `ctx.task().elicit` no longer resumes from an inbound
        // `Response`: MCP 2026-07-28 routes the answer through `tasks/update`,
        // which is a request handled by `App::update_task`. Responses arriving
        // here are ordinary replies to server-initiated requests and go to the
        // request queue unchanged.
        runtime.pending_requests().complete(resp);

        let mut resp = Response::empty(resp_id);
        if let Some(session_id) = session_id {
            resp = resp.set_session_id(session_id);
        }

        resp
    }

    #[inline]
    async fn handle_notification(notification: Notification, runtime: ServerRuntime) {
        match notification.method.as_str() {
            crate::types::notification::commands::CANCELLED => {
                if let Some(params) = notification.params
                    && let Ok(params) =
                        serde_json::from_value::<CancelledNotificationParams>(params)
                {
                    // A cancel aimed at a `subscriptions/listen` ends that
                    // subscription only. Cancelling the request itself would
                    // race the handler for its own response and rob the client
                    // of the graceful-close result.
                    #[cfg(not(feature = "legacy-spec"))]
                    if runtime.options().subscriptions().cancel(&params.request_id) {
                        return;
                    }
                    runtime.options().cancel_request(&params.request_id);
                }
            }
            crate::types::notification::commands::MESSAGE => {
                #[cfg(feature = "tracing")]
                notification.write();
            }
            _ => {}
        }
    }
}

/// Builds the per-request tracing span carrying the session id.
///
/// See the 2026-07-28 variant below for why the span is created at `ERROR`.
#[cfg(all(feature = "tracing", feature = "legacy-spec"))]
fn create_tracing_span(session_id: Option<uuid::Uuid>) -> tracing::Span {
    if let Some(mcp_session_id) = session_id {
        tracing::error_span!("request", mcp_session_id = mcp_session_id.to_string())
    } else {
        tracing::error_span!("request")
    }
}

/// Builds the per-request tracing span, carrying the session id and (2026-07-28) the
/// request-scoped logging level as span fields the notification layer reads.
///
/// The span is created at `ERROR` -- the highest level -- so it is not itself
/// filtered out. It carries no message: it exists only to route and filter the
/// events emitted inside it. At a lower level a common global threshold (say
/// `LevelFilter::WARN`) would disable the span while still letting WARN/ERROR
/// events through, leaving those events with no `mcp_session_id` to route by and
/// no `mcp_log_level` to filter against -- request-scoped logging would silently
/// stop working. `ERROR` keeps the context observable for every event that the
/// application's own threshold admits.
#[cfg(all(feature = "tracing", not(feature = "legacy-spec")))]
fn create_tracing_span(
    session_id: Option<uuid::Uuid>,
    log_level: Option<crate::types::notification::LoggingLevel>,
) -> tracing::Span {
    match (session_id, log_level) {
        (Some(sid), Some(level)) => tracing::error_span!(
            "request",
            mcp_session_id = sid.to_string(),
            mcp_log_level = u64::from(level.severity())
        ),
        (Some(sid), None) => {
            tracing::error_span!("request", mcp_session_id = sid.to_string())
        }
        (None, Some(level)) => {
            tracing::error_span!("request", mcp_log_level = u64::from(level.severity()))
        }
        (None, None) => tracing::error_span!("request"),
    }
}

/// Whether `msg` would open a subscription -- a `subscriptions/listen`
/// request, alone or inside a batch.
///
/// Batches count. Nothing on the server refuses a batched listen: it reaches
/// `App::subscriptions_listen` like any other request, and the HTTP transport
/// holds an acknowledgment permit for it (`is_subscription_stream` there makes
/// the same batch-aware check), so it can register and is owed the same
/// graceful close as any other. Reading only the top-level request would leave
/// exactly that case undrained.
#[cfg(not(feature = "legacy-spec"))]
pub(super) fn opens_subscription(msg: &Message) -> bool {
    fn is_listen(req: &crate::types::Request) -> bool {
        req.method == crate::types::subscription::commands::LISTEN
    }

    match msg {
        Message::Request(req) => is_listen(req),
        Message::Batch(batch) => batch
            .iter()
            .any(|env| matches!(env, MessageEnvelope::Request(req) if is_listen(req))),
        _ => false,
    }
}

/// Counts one message as inside the middleware pipeline for as long as it is
/// held.
///
/// A `Drop` guard rather than a pair of calls so a panicking or cancelled
/// handler cannot leave the count raised -- a leaked count would make the
/// shutdown drain wait out its whole window on every shutdown thereafter.
///
/// The count is what makes "nothing in flight" mean "every response produced so
/// far is already queued on the transport sender": the terminal middleware
/// awaits that send before it returns, so the guard outlives it.
#[cfg(not(feature = "legacy-spec"))]
struct InFlightGuard(Arc<std::sync::atomic::AtomicUsize>);

#[cfg(not(feature = "legacy-spec"))]
impl InFlightGuard {
    #[inline]
    fn enter(counter: Arc<std::sync::atomic::AtomicUsize>) -> Self {
        counter.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Self(counter)
    }
}

#[cfg(not(feature = "legacy-spec"))]
impl Drop for InFlightGuard {
    #[inline]
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

/// Closes the per-request notification sink for a stateless HTTP `POST` when the
/// message's whole middleware pipeline is done (including on panic or task
/// cancellation, hence a `Drop` guard rather than a plain call).
///
/// Dropping the sender is the completion signal a request-scoped SSE `POST`
/// response waits for: only then does it know every notification -- including
/// ones user middleware emitted after `next(ctx)` -- has been queued, so it can
/// drain them and close with the final response.
#[cfg(all(not(feature = "legacy-spec"), feature = "http-server"))]
struct RequestSinkGuard(Option<uuid::Uuid>);

#[cfg(all(not(feature = "legacy-spec"), feature = "http-server"))]
impl Drop for RequestSinkGuard {
    fn drop(&mut self) {
        if let Some(id) = self.0 {
            crate::types::notification::sink::unregister(&id);
        }
    }
}

/// Why a `tools/call`'s mirrored `Mcp-Param-*` headers do not describe its
/// arguments, if they do not.
///
/// A tool may annotate arguments with `x-mcp-header`, and the client must then
/// mirror each present value into `Mcp-Param-{name}`. An intermediary is
/// entitled to route or rate-limit on those headers, so a header that says one
/// tenant while the body says another has to be rejected rather than
/// dispatched -- otherwise the annotation is a suggestion, not a control.
///
/// Each annotated argument is checked in both directions: a value in the body
/// requires the header, a header requires the value, and both present must
/// agree after decoding the Base64 sentinel. Headers naming an argument this
/// tool does not annotate are none of the origin server's business -- the spec
/// has unrecognized `Mcp-Param-*` forwarded and ignored.
///
/// A definition whose annotations are invalid yields no expectations at all:
/// the malformed tool is the problem, and the client is already required to
/// drop it rather than call it.
#[cfg(all(feature = "http-server", not(feature = "legacy-spec")))]
async fn param_header_error(
    req: &Request,
    options: &crate::app::options::RuntimeMcpOptions,
) -> Option<Error> {
    use crate::shared::param_headers;
    use crate::transport::http::decode_header_value;

    if req.method != crate::types::tool::commands::CALL {
        return None;
    }

    // Only a call that arrived as a single HTTP request has mirrored headers to
    // check. `Mcp-Method` is the marker: the transport preamble requires it on
    // every single request and rejects it on a batch, so its absence here means
    // this call came in a batch (or off another transport entirely) and no
    // header ever described it. Demanding one would reject every batched call
    // of an annotated tool.
    if !req.headers.contains_key(crate::transport::http::MCP_METHOD) {
        return None;
    }

    let params = req.params.as_ref()?.as_object()?;
    let name = params.get("name")?.as_str()?;
    let tool = options.get_tool(name).await?;
    let schema = serde_json::to_value(&tool.input_schema).ok()?;
    let declared = param_headers::collect(&schema).ok()?;
    if declared.is_empty() {
        return None;
    }

    let args = params.get("arguments").cloned().unwrap_or_default();
    let mirrored = param_headers::extract(&declared, &args);

    let mismatch = |header: &str, stated: &str, body: &str| {
        Some(Error::new(
            ErrorCode::HeaderMismatch,
            format!(
                "Header mismatch: {header} header value {stated:?} does not match body value {body:?}"
            ),
        ))
    };

    for header in &declared {
        let name = format!("{}{}", param_headers::PARAM_HEADER_PREFIX, header.header);
        let stated = req.headers.get(&name).and_then(|v| v.to_str().ok());
        let body = mirrored
            .iter()
            .find(|(mirrored, _)| *mirrored == name)
            .map(|(_, value)| value.as_str());

        match (stated, body) {
            (None, None) => {}
            (None, Some(body)) => {
                return Some(Error::new(
                    ErrorCode::HeaderMismatch,
                    format!("Missing {name} header for the mirrored argument {body:?}"),
                ));
            }
            (Some(stated), None) => {
                return Some(Error::new(
                    ErrorCode::HeaderMismatch,
                    format!(
                        "{name} header sent as {stated:?}, but the call carries no such argument"
                    ),
                ));
            }
            (Some(stated), Some(body)) => match decode_header_value(stated) {
                Some(decoded) if decoded == body => {}
                Some(decoded) => return mismatch(&name, &decoded, body),
                None => {
                    return Some(Error::new(
                        ErrorCode::HeaderMismatch,
                        format!("Malformed {name} header value"),
                    ));
                }
            },
        }
    }

    None
}

#[cfg(all(test, not(feature = "legacy-spec")))]
mod opens_subscription_tests {
    use super::opens_subscription;
    use crate::types::{Message, MessageBatch, MessageEnvelope, Request, RequestId};

    fn req(method: &str, id: i64) -> Request {
        Request::new(Some(RequestId::Number(id)), method, None::<()>)
    }

    fn listen(id: i64) -> Request {
        req(crate::types::subscription::commands::LISTEN, id)
    }

    #[test]
    fn a_bare_listen_opens_one() {
        assert!(opens_subscription(&Message::Request(listen(1))));
        assert!(!opens_subscription(&Message::Request(req("tools/call", 1))));
    }

    /// Nothing on the server refuses a batched listen, and the HTTP transport
    /// holds an acknowledgment permit for one, so it registers like any other
    /// and is owed the same graceful close. Reading only the top-level request
    /// left exactly that case undrained on shutdown.
    #[test]
    fn a_listen_buried_in_a_batch_opens_one_too() {
        let batch = MessageBatch::new(vec![
            MessageEnvelope::Request(req("tools/call", 1)),
            MessageEnvelope::Request(listen(2)),
        ])
        .expect("non-empty batch");
        assert!(opens_subscription(&Message::Batch(batch)));
    }

    #[test]
    fn a_batch_without_a_listen_opens_nothing() {
        let batch = MessageBatch::new(vec![
            MessageEnvelope::Request(req("tools/call", 1)),
            MessageEnvelope::Request(req("tools/list", 2)),
        ])
        .expect("non-empty batch");
        assert!(!opens_subscription(&Message::Batch(batch)));
    }
}

#[cfg(test)]
mod tests {
    /// The request span carries the routing and filtering context for every
    /// event emitted while handling a request, so a common global threshold
    /// (`LevelFilter::WARN`) must not disable it: WARN/ERROR events stay enabled
    /// and would otherwise lose their `mcp_session_id` and `mcp_log_level`.
    #[cfg(all(feature = "tracing", not(feature = "legacy-spec")))]
    #[test]
    fn request_span_survives_a_warning_only_filter() {
        use crate::types::notification::LoggingLevel;
        use tracing_subscriber::prelude::*;

        let subscriber =
            tracing_subscriber::registry().with(tracing::level_filters::LevelFilter::WARN);

        tracing::subscriber::with_default(subscriber, || {
            let span =
                super::create_tracing_span(Some(uuid::Uuid::new_v4()), Some(LoggingLevel::Warning));
            assert!(
                !span.is_disabled(),
                "the request span must stay enabled under a warning-only filter"
            );
            // The contrast: an INFO span -- what this used to be -- is dropped,
            // taking the request context with it.
            assert!(tracing::info_span!("request").is_disabled());
        });
    }
}

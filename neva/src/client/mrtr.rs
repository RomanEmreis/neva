//! The client half of MRTR (multi-round tool resolution).
//!
//! A server that needs input answers the call with `input_required` instead of
//! a result. [`Client::run_with_mrtr`] is the loop that closes over that: it
//! fulfils what was asked through the configured handlers, echoes the server's
//! `requestState` back byte-exact, and re-sends -- until the call returns a
//! real result or the round budget runs out. `run_batch_with_mrtr` does the
//! same for a batch, where each request advances its own chain.

use super::*;

impl Client {
    /// Picks up `io.modelcontextprotocol/serverInfo` from a result's `_meta`.
    ///
    /// Under MCP 2026-07-28 the server identifies itself on every result rather
    /// than once in a handshake, so the first result that carries it is what
    /// populates [`Self::server_info`].
    #[cfg(not(feature = "legacy-spec"))]
    pub(super) fn record_server_info(&mut self, resp: &Response) {
        if self.server_info.is_some() {
            return;
        }

        let Response::Ok(ok) = resp else { return };
        if let Some(info) = ok
            .result
            .get("_meta")
            .and_then(|m| m.get("io.modelcontextprotocol/serverInfo"))
            .and_then(|v| serde_json::from_value::<Implementation>(v.clone()).ok())
        {
            self.server_info = Some(info);
        }
    }

    /// Sends a request and transparently drives the MRTR loop: while the
    /// server responds with an `input_required` result, fulfil each
    /// elicitation via the configured handler and re-issue the original
    /// request (new id) with `inputResponses` + the echoed `requestState`.
    #[cfg(not(feature = "legacy-spec"))]
    pub(super) async fn run_with_mrtr(&mut self, req: Request) -> Result<Response, Error> {
        let max_rounds = self.options.max_mrtr_rounds;
        let method = req.method.clone();
        let original_params = req.params.clone();
        let mrtr_method = shared::is_mrtr_method(method.as_str());

        let mut req = req;
        self.apply_client_meta(&mut req, None, None);

        // The budget counts re-issue rounds, not the initial send, so allow the
        // first attempt plus `max_rounds` retries. `0..=max_rounds` (vs.
        // `max_rounds + 1`) also avoids overflow at `usize::MAX`.
        for _ in 0..=max_rounds {
            let resp = self
                .handler
                .as_mut()
                .ok_or_else(|| Error::new(ErrorCode::InternalError, "Connection closed"))?
                .send_request(req)
                .await?;

            self.record_server_info(&resp);

            // MRTR only applies to success results carrying the
            // `input_required` discriminator; anything else -- including a
            // result with no `resultType` at all -- is final.
            let input_required_result = match &resp {
                Response::Ok(ok)
                    if mrtr_method
                        && resp.result_type() == Some(crate::types::ResultType::InputRequired) =>
                {
                    serde_json::from_value::<crate::types::mrtr::InputRequiredResult>(
                        ok.result.clone(),
                    )
                    .map_err(Error::from)?
                }
                _ => return Ok(resp),
            };
            let ir = input_required_result;

            let mut input_responses = crate::types::mrtr::InputResponses::new();
            if let Some(reqs) = ir.input_requests {
                for (key, request) in reqs {
                    input_responses.insert(key, self.fulfil_input(request).await?);
                }
            }

            let new_id = self.generate_id()?;
            let mut retry = Request::new(Some(new_id), method.clone(), original_params.clone());
            self.apply_client_meta(&mut retry, Some(input_responses), ir.request_state);
            req = retry;
        }

        Err(Error::new(
            ErrorCode::InternalError,
            "MRTR exceeded the maximum number of rounds",
        ))
    }

    /// Sets `clientInfo` + MRTR capability `_meta` on a request, and optionally
    /// the MRTR `inputResponses` / `requestState`, preserving existing `_meta`.
    ///
    /// Also populates W3C Trace Context (`traceparent` / `tracestate`) from the
    /// configured [`trace_context_provider`](crate::client::options::McpOptions::with_trace_context_provider),
    /// when installed. This is the single assembly point for outbound 2026-07-28 `_meta`,
    /// so both single sends (via [`Self::run_with_mrtr`]) and batched requests
    /// (via [`Self::run_batch_with_mrtr`]) carry trace context.
    #[cfg(not(feature = "legacy-spec"))]
    pub(super) fn apply_client_meta(
        &self,
        req: &mut Request,
        input_responses: Option<crate::types::mrtr::InputResponses>,
        request_state: Option<String>,
    ) {
        let mut meta = req.meta().unwrap_or_default();
        meta.client_info = Some(self.options.implementation.clone());
        // Required on every request under MCP 2026-07-28, and it must agree
        // with the `MCP-Protocol-Version` header the HTTP transport sets.
        meta.protocol_version = Some(self.expected_protocol_ver().to_string());
        // Each flag reflects what this client can actually fulfil right now: a
        // configured handler for elicitation/sampling, and -- since roots are
        // data rather than a handler -- a declared roots capability. That is
        // either an explicit `with_roots(..)` or simply having roots, and it
        // deliberately stays true for an empty list: an empty
        // `ListRootsResult` is a valid answer, so a client that opted in must
        // not be gated out of being asked.
        meta.client_capabilities = Some(crate::types::mrtr::ClientMrtrCapabilities {
            // Declared without naming modes, which is the honest answer: the
            // handler is handed the whole `ElicitRequestParams` union, so what
            // it does with a `url` request is the caller's business and not
            // something this client can promise on its behalf. An unstated set
            // rules nothing out, which is exactly that.
            elicitation: self
                .options
                .elicitation_handler
                .is_some()
                .then(crate::types::mrtr::ElicitationModes::default),
            sampling: self.options.sampling_handler.is_some(),
            roots: self.options.roots_capability().is_some(),
        });

        if let Some(provider) = self.options.trace_context_provider.as_ref()
            && let Some(tc) = provider()
        {
            meta.traceparent = Some(tc.traceparent);
            meta.tracestate = tc.tracestate;
            meta.baggage = tc.baggage;
        }

        // Request-scoped logging level (replaces the removed `logging/setLevel`).
        if self.options.log_level.is_some() {
            meta.log_level = self.options.log_level;
        }

        req.set_meta(meta);

        // The MRTR re-run fields are *params*, not metadata: the spec puts
        // `inputResponses` and `requestState` on `InputResponseRequestParams`,
        // next to `name` / `arguments`. They are written after `set_meta` so
        // neither can clobber the other -- both edit the same params object.
        if input_responses.is_some() || request_state.is_some() {
            let mut params = match req.params.take() {
                Some(serde_json::Value::Object(map)) => map,
                // A retry of a request that carried no params at all still has
                // somewhere to put the answers.
                _ => serde_json::Map::new(),
            };

            if let Some(responses) = input_responses
                && let Ok(value) = serde_json::to_value(responses)
            {
                params.insert("inputResponses".into(), value);
            }

            if let Some(state) = request_state {
                params.insert("requestState".into(), serde_json::Value::String(state));
            }

            req.params = Some(serde_json::Value::Object(params));
        }
    }

    /// Applies the initial per-request 2026-07-28 client metadata (`clientInfo` /
    /// `clientCapabilities`, plus trace context) to every [`Request`] in a
    /// batch. The MRTR re-run fields (`inputResponses` / `requestState`) stay
    /// `None` here -- they are filled per request on each retry round by
    /// [`Self::run_batch_with_mrtr`]. Notifications are left untouched.
    #[cfg(not(feature = "legacy-spec"))]
    pub(super) fn apply_client_meta_to_batch(&self, items: &mut [MessageEnvelope]) {
        for envelope in items {
            if let MessageEnvelope::Request(req) = envelope {
                self.apply_client_meta(req, None, None);
            }
        }
    }

    /// Fulfils one server-requested input, whatever its kind, and returns the
    /// raw result to echo back under the request's key.
    ///
    /// Sampling and roots are fulfilled here, on the MRTR loop -- *not* as
    /// server-initiated pushes: under MCP 2026-07-28 there is no such channel. The
    /// client only ever gets asked for a kind it declared in
    /// [`ClientMrtrCapabilities`](crate::types::mrtr::ClientMrtrCapabilities),
    /// so a missing handler here means the server ignored those flags.
    #[cfg(not(feature = "legacy-spec"))]
    pub(super) async fn fulfil_input(
        &self,
        request: crate::types::mrtr::InputRequest,
    ) -> Result<serde_json::Value, Error> {
        use crate::types::mrtr::InputRequest;

        #[allow(deprecated)]
        let value = match request {
            InputRequest::Elicitation(params) => match self.options.elicitation_handler.clone() {
                Some(handler) => serde_json::to_value(handler(params).await)?,
                None => return Err(no_fulfiller("elicitation")),
            },
            InputRequest::Sampling(params) => match self.options.sampling_handler.clone() {
                Some(handler) => serde_json::to_value(handler(*params).await)?,
                None => return Err(no_fulfiller("sampling")),
            },
            // Roots are configured data, not a handler: the client answers
            // from the list it was built with.
            InputRequest::Roots(_) => serde_json::to_value(crate::types::root::ListRootsResult {
                roots: self.options.roots(),
                meta: None,
            })?,
        };

        Ok(value)
    }

    /// Drives the MRTR retry loop across an entire batch.
    ///
    /// Each batched [`Request`] that elicits -- the server replies with an
    /// `input_required` result -- is fulfilled via the configured elicitation
    /// handler and re-issued (carrying `inputResponses` + the echoed
    /// `requestState`) alongside any other still-eliciting requests, so the
    /// whole batch is driven to completion in lock-step rounds. One transport
    /// write per round preserves the batching benefit; final
    /// (non-`input_required`) responses are retained and not re-sent. Each
    /// request keeps its slot in the returned `Vec`, in input order;
    /// notifications (and any non-request envelopes) are sent once, in the
    /// first round, and produce no slot.
    #[cfg(not(feature = "legacy-spec"))]
    pub(super) async fn run_batch_with_mrtr(
        &mut self,
        items: Vec<MessageEnvelope>,
    ) -> Result<Vec<Response>, Error> {
        let max_rounds = self.options.max_mrtr_rounds;

        // A per-request slot: either still eliciting (carrying what is needed to
        // re-issue) or resolved to its final response.
        enum Slot {
            Pending {
                method: String,
                original_params: Option<serde_json::Value>,
                req: Option<Request>,
            },
            Done(Response),
        }

        // Seed round-0 metadata (`clientInfo` / `clientCapabilities` / trace
        // context) on every request via the shared assembly path, then split
        // requests (which get ordered slots) from fire-and-forget extras
        // (notifications) sent only in the first round.
        let mut items = items;
        self.apply_client_meta_to_batch(&mut items);

        let mut slots: Vec<Slot> = Vec::new();
        let mut extras: Vec<MessageEnvelope> = Vec::new();
        for envelope in items {
            match envelope {
                MessageEnvelope::Request(req) => slots.push(Slot::Pending {
                    method: req.method.clone(),
                    original_params: req.params.clone(),
                    req: Some(req),
                }),
                other => extras.push(other),
            }
        }

        // No requests to drive through MRTR: send any extras (notifications)
        // once and let `send_batch` surface the same connection-closed /
        // empty-batch errors a non-eliciting batch would.
        if slots.is_empty() {
            let handler = self
                .handler
                .as_mut()
                .ok_or_else(|| Error::new(ErrorCode::InternalError, "Connection closed"))?;
            let request_timeout = handler.timeout();
            let pending = handler.pending().clone();
            let token = handler.cancellation();
            let receivers = handler.send_batch(extras).await?;
            return collect_batch_responses(receivers, &pending, request_timeout, token)
                .await
                .into_iter()
                .collect();
        }

        // Round 0 is the initial batch send; rounds `1..=max_rounds` are the
        // re-issues the budget allows (the cap counts retries, not the first
        // send). `0..=max_rounds` also avoids overflow at `usize::MAX`.
        for round in 0..=max_rounds {
            // Collect this round's outgoing requests; notifications ride along once.
            let mut envelopes: Vec<MessageEnvelope> = Vec::new();
            if round == 0 {
                envelopes.append(&mut extras);
            }

            let mut round_slots: Vec<usize> = Vec::new();
            for (i, slot) in slots.iter_mut().enumerate() {
                if let Slot::Pending { req, .. } = slot
                    && let Some(request) = req.take()
                {
                    round_slots.push(i);
                    envelopes.push(MessageEnvelope::Request(request));
                }
            }

            if round_slots.is_empty() {
                break;
            }

            // One transport write; await this round's replies concurrently.
            let handler = self
                .handler
                .as_mut()
                .ok_or_else(|| Error::new(ErrorCode::InternalError, "Connection closed"))?;

            let request_timeout = handler.timeout();
            let pending = handler.pending().clone();
            let token = handler.cancellation();
            let receivers = handler.send_batch(envelopes).await?;
            let responses =
                collect_batch_responses(receivers, &pending, request_timeout, token).await;

            // `responses` aligns with `round_slots`: `send_batch` preserves
            // request order and extras produce no receiver. Final responses fill
            // their slot; `input_required` ones are fulfilled and re-issued.
            for (slot_i, resp) in round_slots.into_iter().zip(responses) {
                let resp = resp?;
                self.record_server_info(&resp);
                let (method, original_params) = match &slots[slot_i] {
                    Slot::Pending {
                        method,
                        original_params,
                        ..
                    } => (method.clone(), original_params.clone()),
                    Slot::Done(_) => unreachable!("a round slot is always pending"),
                };

                let is_input_required = shared::is_mrtr_method(method.as_str())
                    && resp.result_type() == Some(crate::types::ResultType::InputRequired);

                if !is_input_required {
                    slots[slot_i] = Slot::Done(resp);
                    continue;
                }

                let ir = match &resp {
                    Response::Ok(ok) => serde_json::from_value::<
                        crate::types::mrtr::InputRequiredResult,
                    >(ok.result.clone())
                    .map_err(Error::from)?,
                    Response::Err(_) => unreachable!("input_required is a success result"),
                };

                let mut input_responses = crate::types::mrtr::InputResponses::new();
                if let Some(reqs) = ir.input_requests {
                    for (key, request) in reqs {
                        input_responses.insert(key, self.fulfil_input(request).await?);
                    }
                }

                let new_id = self.generate_id()?;
                let mut retry = Request::new(Some(new_id), method, original_params);

                self.apply_client_meta(&mut retry, Some(input_responses), ir.request_state);

                if let Slot::Pending { req, .. } = &mut slots[slot_i] {
                    *req = Some(retry);
                }
            }
        }

        // Assemble in slot order; a slot still pending exhausted the rounds.
        let mut out = Vec::with_capacity(slots.len());
        for slot in slots {
            match slot {
                Slot::Done(resp) => out.push(resp),
                Slot::Pending { .. } => {
                    return Err(Error::new(
                        ErrorCode::InternalError,
                        "MRTR exceeded the maximum number of rounds",
                    ));
                }
            }
        }
        Ok(out)
    }
}

/// The error for an input kind the server asked for but this client has no
/// fulfiller for -- only reachable if the server ignored the declared
/// [`ClientMrtrCapabilities`](crate::types::mrtr::ClientMrtrCapabilities).
#[cfg(not(feature = "legacy-spec"))]
fn no_fulfiller(kind: &str) -> Error {
    Error::new(
        ErrorCode::InvalidRequest,
        format!("server requested {kind} but no handler is configured"),
    )
}

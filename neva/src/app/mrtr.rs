//! MRTR (multi-round tool resolution) plumbing for the request pipeline.
//!
//! The round trip is stateless: everything a chain needs in order to resume
//! lives in the sealed `requestState` the previous round handed the caller.
//! These are the four points the server touches it -- what identifies a request
//! across rounds, what is verified on the way in, what is minted on the way
//! out, and when a round's deferred effects may commit.
//!
//! See [`crate::types::mrtr`] for the state codec and why it seals rather than
//! signs.

use crate::error::{Error, ErrorCode};
use crate::types::{Request, Response};

/// Returns a clone of `params` with everything that differs between MRTR
/// rounds removed, so the request-binding digest is stable across round-trips.
///
/// That is `_meta`, plus the two fields the retry adds -- `inputResponses` and
/// `requestState`. Leaving those in would make round 2 hash differently from
/// round 1 and every state would be rejected as "not matching this request":
/// the binding is about *which* request this is, and a request does not become
/// a different one by being answered.
pub(super) fn salient_params(params: &serde_json::Value) -> serde_json::Value {
    match params {
        serde_json::Value::Object(map) => {
            let mut cloned = map.clone();
            cloned.remove("_meta");
            cloned.remove("inputResponses");
            cloned.remove("requestState");
            serde_json::Value::Object(cloned)
        }
        other => other.clone(),
    }
}

/// Decodes/verifies any incoming `requestState` and merges this round's
/// `inputResponses` into the replay log, producing the per-dispatch MRTR state.
pub(super) fn seed_mrtr_ctx(
    req: &Request,
    method: &str,
    salient: &serde_json::Value,
    options: &crate::app::options::RuntimeMcpOptions,
    principal: Option<&str>,
) -> Result<std::sync::Arc<crate::app::context::MrtrCtx>, Error> {
    use crate::types::mrtr::state::{StateCodec, now_secs, request_binding};

    let meta = req.meta();
    let client_capabilities = meta
        .as_ref()
        .and_then(|m| m.client_capabilities)
        .unwrap_or_default();

    let mut answers = std::collections::HashMap::new();
    let mut memos = std::collections::HashMap::new();
    let mut effects = std::collections::HashSet::new();
    // Keys the server requested in the prior round, decoded from the verified
    // state. `None` means no valid state was supplied, so no input was solicited.
    let mut requested: Option<Vec<String>> = None;

    if let Some(state) = req.state() {
        // Reject an oversized inbound state before decoding it. Base64 decoding
        // and AEAD decryption in `StateCodec::decode` both allocate/compute in
        // proportion to the blob size, so without this guard `with_max_state_bytes`
        // would only bound the states we *mint* and a bogus oversized
        // `requestState` from an untrusted client could force that work before
        // failing. The cap is the same encoded-length bound enforced on the
        // outbound path in `build_input_required`.
        if state.len() > options.max_state_bytes() {
            return Err(Error::new(
                ErrorCode::InvalidParams,
                "requestState exceeds the configured maximum size",
            ));
        }

        let payload = StateCodec::new(options.request_state_keys()).decode(&state)?;
        if payload.exp < now_secs() {
            return Err(Error::new(ErrorCode::InvalidParams, "requestState expired"));
        }

        if payload.req != request_binding(method, salient) {
            return Err(Error::new(
                ErrorCode::InvalidParams,
                "requestState does not match this request",
            ));
        }

        if payload.principal.as_deref() != principal {
            return Err(Error::new(
                ErrorCode::InvalidParams,
                "requestState principal mismatch",
            ));
        }

        // Which service minted it. Compared both ways, like the principal
        // above: a state that names an audience is refused by a server that
        // configures none, so the binding cannot be shed by dropping the claim
        // on the way back in.
        if payload.aud.as_deref() != options.request_state_audience() {
            return Err(Error::new(
                ErrorCode::InvalidParams,
                "requestState audience mismatch",
            ));
        }

        answers = payload.answers;
        memos = payload.memos;
        effects = payload.effects;
        requested = Some(payload.requested);
    }
    if let Some(responses) = req.input_responses() {
        // An answer is taken when it fits what this chain is waiting for, and
        // dropped otherwise. Nothing here is an error: the spec has a server
        // ignore information it does not recognize, and a client that re-sends
        // its whole answer set every round -- a perfectly ordinary client -- is
        // not misbehaving. What a dropped answer costs the client is a round: the
        // handler asks again, and the fresh `InputRequiredResult` says what for.
        for (key, value) in responses {
            // An answer already sealed into the state is settled. A later one
            // for the same key is not an update; honoring it would let a replay
            // of one round's state carry a different answer than the round that
            // produced it.
            if answers.contains_key(&key) {
                continue;
            }
            // With a verified state we know exactly what was asked, so anything
            // else is unsolicited -- including an answer pre-seeded for a key
            // the handler has not reached yet, which would skip the elicitation
            // the server intended to make. Without a state there is nothing to
            // check against and no round in flight to subvert, so an answer
            // offered up front is simply available to the handler that asks for
            // it.
            if requested.as_ref().is_some_and(|r| !r.contains(&key)) {
                continue;
            }

            answers.insert(key, value);
        }
    }

    Ok(std::sync::Arc::new(crate::app::context::MrtrCtx {
        answers,
        pending: Default::default(),
        client_capabilities,
        malformed_answer: Default::default(),
        memos: std::sync::Mutex::new(memos),
        effects: std::sync::Mutex::new(effects),
        commits: Default::default(),
    }))
}

/// Builds the `InputRequiredResult` for the input the handler requested,
/// encoding a fresh encrypted `requestState`.
pub(super) fn build_input_required(
    arc: &std::sync::Arc<crate::app::context::MrtrCtx>,
    method: &str,
    salient: &serde_json::Value,
    options: &crate::app::options::RuntimeMcpOptions,
    principal: Option<String>,
) -> Result<crate::types::mrtr::InputRequiredResult, Error> {
    use crate::types::mrtr::InputRequiredResult;
    use crate::types::mrtr::state::{StateCodec, StatePayload, now_secs, request_binding};

    let pending = arc
        .pending
        .lock()
        .map(|mut p| std::mem::take(&mut *p))
        .unwrap_or_default();

    if pending.is_empty() {
        return Err(Error::new(
            ErrorCode::InternalError,
            "missing pending MRTR input",
        ));
    }

    // Each kind is gated on its own flag: a client that fulfils elicitation
    // need not fulfil the deprecated sampling/roots kinds, and asking for one
    // it never declared would otherwise stall the round-trip. One unsupported
    // kind fails the round even when the others are fine -- a partial round
    // would silently drop an input the handler is going to ask for again.
    if let Some((_, request)) = pending
        .iter()
        .find(|(_, request)| !arc.client_capabilities.allows(request))
    {
        return Err(Error::new(
            ErrorCode::MissingRequiredClientCapability,
            format!(
                "server requested `{}` but the client did not declare support",
                request.method()
            ),
        )
        .with_data(serde_json::json!({
            "requiredCapabilities": arc.client_capabilities.requiring(request),
        })));
    }

    let memos = arc.memos.lock().map(|m| m.clone()).unwrap_or_default();
    let effects = arc.effects.lock().map(|e| e.clone()).unwrap_or_default();

    let payload = StatePayload {
        answers: arc.answers.clone(),
        // Bind the keys we are requesting into the signed state so the next
        // round can tell an answer it asked for from one it did not.
        requested: pending.iter().map(|(key, _)| key.clone()).collect(),
        memos,
        effects,
        exp: now_secs() + options.request_state_ttl_secs(),
        req: request_binding(method, salient),
        principal,
        aud: options.request_state_audience().map(str::to_owned),
    };

    let state = StateCodec::new(options.request_state_keys()).encode(&payload)?;
    if state.len() > options.max_state_bytes() {
        return Err(Error::new(
            ErrorCode::InternalError,
            "requestState too large",
        ));
    }

    Ok(InputRequiredResult::new(pending, state))
}

/// Returns `true` only when `resp` is a genuine success that should trigger the
/// final round's deferred MRTR commits.
///
/// A protocol-level failure (`Err`, or an `Ok(Response::Err(..))`) is excluded
/// by construction. Crucially, so is an *in-band* tool error: tool/prompt
/// wrappers fold a handler `Err` into `Ok(CallToolResponse { isError: true })`,
/// so a plain `resp.is_ok()` check would still run commits on a failed call --
/// applying irreversible side effects (DB writes, charges) registered via
/// `ctx.on_commit(..)` even though the tool ultimately reported failure.
pub(super) fn mrtr_should_commit(resp: &Result<Response, Error>) -> bool {
    match resp {
        Ok(Response::Ok(ok)) => ok.result.get("isError") != Some(&serde_json::Value::Bool(true)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deferred MRTR commits must run only for a genuine success -- never for a
    /// protocol-level error nor for an in-band tool error (`isError: true`),
    /// which tool wrappers fold a handler `Err` into.
    #[test]
    fn mrtr_should_commit_excludes_errors() {
        use crate::error::Error;
        use crate::types::{RequestId, Response};
        use serde_json::json;

        let id = RequestId::Number(1);

        // Genuine success -> commit.
        let ok = Ok(Response::success(
            id.clone(),
            json!({ "content": [], "isError": false }),
        ));
        assert!(mrtr_should_commit(&ok));

        // Success with no `isError` field at all (e.g. a non-tool result) -> commit.
        let plain = Ok(Response::success(id.clone(), json!({ "ok": true })));
        assert!(mrtr_should_commit(&plain));

        // In-band tool error folded into Ok -> do NOT commit.
        let tool_err = Ok(Response::success(
            id.clone(),
            json!({ "content": [], "isError": true }),
        ));
        assert!(!mrtr_should_commit(&tool_err));

        // Protocol-level error response -> do NOT commit.
        let proto_err = Ok(Response::error(id.clone(), Error::new(-32603, "boom")));
        assert!(!mrtr_should_commit(&proto_err));

        // Handler `Err` -> do NOT commit.
        let hard_err: Result<Response, Error> = Err(Error::new(-32603, "boom"));
        assert!(!mrtr_should_commit(&hard_err));
    }

    /// Security guards in [`super::seed_mrtr_ctx`] that the e2e happy path never
    /// exercises: an expired `requestState` and a principal-bound state replayed
    /// under a different principal. Driven deterministically (no clock advance,
    /// no auth harness) by hand-encoding the signed blob.
    mod mrtr_seed_guards {
        use crate::app::App;
        use crate::error::ErrorCode;
        use crate::types::mrtr::state::{StateCodec, StatePayload, now_secs, request_binding};
        use crate::types::{Request, RequestId};

        const SECRET: &[u8] = b"unit-secret";
        const METHOD: &str = "tools/call";

        fn options() -> crate::app::options::RuntimeMcpOptions {
            App::new()
                .with_request_state_secret(SECRET)
                .options
                .into_runtime()
        }

        fn salient() -> serde_json::Value {
            serde_json::json!({ "name": "greet", "arguments": {} })
        }

        fn request_with_state(state: &str) -> Request {
            let mut params = salient();
            params["_meta"] = serde_json::json!({
                "requestState": state,
                "io.modelcontextprotocol/clientCapabilities": { "elicitation": true }
            });
            Request::new(Some(RequestId::Number(1)), METHOD, Some(params))
        }

        fn encode(payload: &StatePayload) -> String {
            let ring = crate::types::mrtr::state::StateKeyring::single(SECRET);
            StateCodec::new(&ring).encode(payload).expect("encode")
        }

        #[test]
        fn expired_request_state_is_rejected() {
            let payload = StatePayload {
                answers: Default::default(),
                requested: Default::default(),
                memos: Default::default(),
                effects: Default::default(),
                exp: now_secs().saturating_sub(1), // already in the past
                req: request_binding(METHOD, &salient()),
                principal: None,
                aud: None,
            };
            let req = request_with_state(&encode(&payload));
            let err = super::super::seed_mrtr_ctx(&req, METHOD, &salient(), &options(), None)
                .expect_err("expired state must be rejected");
            assert_eq!(err.code, ErrorCode::InvalidParams);
            assert!(format!("{err}").contains("expired"), "{err}");
        }

        #[test]
        fn principal_mismatch_is_rejected() {
            // State minted for "alice" (valid, unexpired, correctly bound)...
            let payload = StatePayload {
                answers: Default::default(),
                requested: Default::default(),
                memos: Default::default(),
                effects: Default::default(),
                exp: now_secs() + 300,
                req: request_binding(METHOD, &salient()),
                principal: Some("alice".into()),
                aud: None,
            };
            let req = request_with_state(&encode(&payload));
            // ...replayed by "bob".
            let err =
                super::super::seed_mrtr_ctx(&req, METHOD, &salient(), &options(), Some("bob"))
                    .expect_err("principal mismatch must be rejected");
            assert_eq!(err.code, ErrorCode::InvalidParams);
            assert!(format!("{err}").contains("principal mismatch"), "{err}");
        }

        const AUDIENCE: &str = "https://weather.example.com/mcp";

        fn options_for(audience: &str) -> crate::app::options::RuntimeMcpOptions {
            App::new()
                .with_request_state_secret(SECRET)
                .with_request_state_audience(audience)
                .options
                .into_runtime()
        }

        fn payload_for(audience: Option<&str>) -> StatePayload {
            StatePayload {
                answers: Default::default(),
                requested: Default::default(),
                memos: Default::default(),
                effects: Default::default(),
                exp: now_secs() + 300,
                req: request_binding(METHOD, &salient()),
                principal: None,
                aud: audience.map(str::to_owned),
            }
        }

        /// Two services sharing a `requestState` secret can decrypt each
        /// other's states, and a method and parameters they both serve make one
        /// of them a state the other would otherwise accept mid-flow. The
        /// audience is what tells them apart.
        #[test]
        fn a_state_minted_for_another_service_is_rejected() {
            let req = request_with_state(&encode(&payload_for(Some(
                "https://billing.example.com/mcp",
            ))));
            let err =
                super::super::seed_mrtr_ctx(&req, METHOD, &salient(), &options_for(AUDIENCE), None)
                    .expect_err("a state minted for another service must be rejected");
            assert_eq!(err.code, ErrorCode::InvalidParams);
            assert!(format!("{err}").contains("audience mismatch"), "{err}");
        }

        #[test]
        fn a_state_minted_for_this_service_is_accepted() {
            let req = request_with_state(&encode(&payload_for(Some(AUDIENCE))));
            assert!(
                super::super::seed_mrtr_ctx(&req, METHOD, &salient(), &options_for(AUDIENCE), None)
                    .is_ok()
            );
        }

        /// Checked in both directions, like the principal guard. A payload
        /// predating the field decodes with no audience, and a server that
        /// demands one refuses it -- so the binding cannot be shed by dropping
        /// the claim, whether the state is old or forged.
        #[test]
        fn an_unbound_state_is_refused_where_an_audience_is_demanded() {
            let req = request_with_state(&encode(&payload_for(None)));
            let err =
                super::super::seed_mrtr_ctx(&req, METHOD, &salient(), &options_for(AUDIENCE), None)
                    .expect_err("an unbound state must not pass an audience check");
            assert!(format!("{err}").contains("audience mismatch"), "{err}");
        }

        /// And the other way: a server that configures no audience refuses a
        /// state that names one, rather than treating the claim as decoration.
        #[test]
        fn a_bound_state_is_refused_where_no_audience_is_configured() {
            let req = request_with_state(&encode(&payload_for(Some(AUDIENCE))));
            let err = super::super::seed_mrtr_ctx(&req, METHOD, &salient(), &options(), None)
                .expect_err("a bound state must not pass an unbound server");
            assert!(format!("{err}").contains("audience mismatch"), "{err}");
        }

        /// An accepted elicitation answer tagged so two answers for one key can
        /// be told apart.
        fn answer(tag: &str) -> serde_json::Value {
            serde_json::json!({ "action": "accept", "content": { "tag": tag } })
        }

        /// The tag [`answer`] put on the answer stored under `key`.
        fn tag_of(ctx: &crate::app::context::MrtrCtx, key: &str) -> Option<String> {
            ctx.answers.get(key)?["content"]["tag"]
                .as_str()
                .map(str::to_owned)
        }

        /// Builds the `_meta` JSON with the given request state (if any) and
        /// `inputResponses` map of key -> tagged answer.
        fn request_with_answers(state: Option<&str>, responses: &[(&str, &str)]) -> Request {
            let responses: serde_json::Map<String, serde_json::Value> = responses
                .iter()
                .map(|(key, tag)| ((*key).to_owned(), answer(tag)))
                .collect();
            let mut meta = serde_json::json!({
                "io.modelcontextprotocol/clientCapabilities": { "elicitation": true },
                "inputResponses": responses,
            });
            if let Some(state) = state {
                meta["requestState"] = serde_json::json!(state);
            }
            let mut params = salient();
            params["_meta"] = meta;
            Request::new(Some(RequestId::Number(1)), METHOD, Some(params))
        }

        fn state_with(answers: &[(&str, &str)], requested: &[&str]) -> String {
            let answers = answers
                .iter()
                .map(|(key, tag)| ((*key).to_owned(), answer(tag)))
                .collect();
            let payload = StatePayload {
                answers,
                requested: requested.iter().map(|k| (*k).to_owned()).collect(),
                memos: Default::default(),
                effects: Default::default(),
                exp: now_secs() + 300,
                req: request_binding(METHOD, &salient()),
                principal: None,
                aud: None,
            };
            encode(&payload)
        }

        #[test]
        fn answers_offered_without_a_request_state_are_available_to_the_handler() {
            // Nothing is in flight, so there is no round to subvert: an answer
            // offered up front is simply there when the handler asks for that
            // key. Erroring instead would break a client that knows what the
            // tool will ask and answers in one shot.
            let req = request_with_answers(None, &[("ask_name", "offered")]);
            let ctx = super::super::seed_mrtr_ctx(&req, METHOD, &salient(), &options(), None)
                .expect("unsolicited answers must not fail the call");
            assert_eq!(tag_of(&ctx, "ask_name").as_deref(), Some("offered"));
        }

        #[test]
        fn an_unsolicited_key_is_dropped_once_a_state_says_what_was_asked() {
            // State requested `ask_name`; the client answers that plus a key
            // nobody asked about. The extra one is ignored rather than fatal --
            // a server SHOULD ignore what it does not recognize -- and, more to
            // the point, is not left lying around to satisfy a later
            // `ctx.elicit("ask_age", ..)` the server has not made yet.
            let state = state_with(&[], &["ask_name"]);
            let req = request_with_answers(
                Some(&state),
                &[("ask_name", "solicited"), ("ask_age", "unsolicited")],
            );
            let ctx = super::super::seed_mrtr_ctx(&req, METHOD, &salient(), &options(), None)
                .expect("an unsolicited key must not fail the call");
            assert_eq!(tag_of(&ctx, "ask_name").as_deref(), Some("solicited"));
            assert!(!ctx.answers.contains_key("ask_age"));
        }

        #[test]
        fn a_settled_answer_is_not_overwritten_by_a_later_one() {
            // `ask_name` is already sealed into the signed answers log. A second
            // answer for it is dropped: taking it would let one round's state be
            // replayed carrying a different answer than the round that made it.
            let state = state_with(&[("ask_name", "settled")], &["ask_name"]);
            let req = request_with_answers(Some(&state), &[("ask_name", "overwrite")]);
            let ctx = super::super::seed_mrtr_ctx(&req, METHOD, &salient(), &options(), None)
                .expect("a repeated answer must not fail the call");
            assert_eq!(tag_of(&ctx, "ask_name").as_deref(), Some("settled"));
        }

        #[test]
        fn solicited_input_response_is_accepted() {
            // The happy path: client answers exactly the requested key.
            let state = state_with(&[], &["ask_name"]);
            let req = request_with_answers(Some(&state), &[("ask_name", "answered")]);
            let ctx = super::super::seed_mrtr_ctx(&req, METHOD, &salient(), &options(), None)
                .expect("solicited response must be accepted");
            assert_eq!(tag_of(&ctx, "ask_name").as_deref(), Some("answered"));
        }
    }
}

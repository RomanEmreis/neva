//! Per-round MRTR bookkeeping, and the three helpers a handler uses to survive
//! being re-run.
//!
//! An MRTR handler runs again from the top on every round, so anything it did
//! last round it would do again. [`Context::once`] and [`Context::memo`] make a
//! side effect or a computed value survive the re-run by recording it in the
//! sealed `requestState`; [`Context::on_commit`] goes the other way and defers
//! an effect until the round that actually finishes. [`MrtrCtx`] is where all of
//! that is kept for the duration of one dispatch.

use super::*;

/// Boxed deferred-commit future (see [`Context::on_commit`]).
#[cfg(not(feature = "legacy-spec"))]
pub(crate) type CommitFut = std::pin::Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>;

/// Per-dispatch MRTR state: the replay log of answers available to the
/// handler this round, the single input it newly requested, plus the
/// `once`/`memo`/`on_commit` bookkeeping.
#[cfg(not(feature = "legacy-spec"))]
#[derive(Default)]
pub(crate) struct MrtrCtx {
    /// Answers available this round (prior answers decoded from
    /// `requestState`, merged with this round's `inputResponses`).
    ///
    /// Raw [`serde_json::Value`]s: the result type depends on which kind of
    /// input was requested, so each helper deserializes its own -- the same
    /// arrangement [`Context::memo`] uses.
    pub(crate) answers: HashMap<String, serde_json::Value>,

    /// The inputs newly requested this round, in the order they were asked for,
    /// each recorded on a cache miss.
    ///
    /// A handler that unwinds on its first miss (the usual `ctx.elicit(..).await?`)
    /// leaves one here and spends a round on it. One that holds its `?` until it
    /// has asked for everything leaves several, and they travel in a single
    /// `InputRequiredResult` -- one round-trip instead of one per input.
    pub(crate) pending: std::sync::Mutex<Vec<(String, crate::types::mrtr::InputRequest)>>,

    /// Which input-request kinds the client declared support for this round.
    pub(crate) client_capabilities: crate::types::mrtr::ClientMrtrCapabilities,

    /// Why an answer could not be read as the kind it answers, if one could not.
    ///
    /// Recorded rather than merely returned because the handler's `Err` does not
    /// survive: tool and prompt wrappers fold it into an in-band error result,
    /// which on the wire is a *complete* result and reads as the call having run
    /// and failed. A malformed answer is the client getting the protocol wrong,
    /// and the dispatch layer promotes this back to a JSON-RPC error so it says
    /// so. Same reason [`Self::pending`] is recorded instead of inferred.
    pub(crate) malformed_answer: std::sync::Mutex<Option<String>>,

    /// Cached `ctx.memo` values (seeded from `requestState`, grown on miss).
    pub(crate) memos: std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>,

    /// Executed `ctx.once` keys (seeded from `requestState`, grown on run).
    pub(crate) effects: std::sync::Mutex<std::collections::HashSet<String>>,

    /// Deferred `ctx.on_commit` futures, rebuilt each round, drained on the
    /// final (non-`input_required`) round. Never serialized.
    pub(crate) commits: std::sync::Mutex<Vec<CommitFut>>,
}

#[cfg(not(feature = "legacy-spec"))]
impl std::fmt::Debug for MrtrCtx {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MrtrCtx")
            .field("answers", &self.answers)
            .field("pending", &self.pending)
            .field("client_capabilities", &self.client_capabilities)
            .field("malformed_answer", &self.malformed_answer)
            .field("memos", &self.memos)
            .field("effects", &self.effects)
            .finish_non_exhaustive()
    }
}

#[cfg(not(feature = "legacy-spec"))]
impl MrtrCtx {
    /// Returns the cached answer for `key`, or records the request and returns
    /// the MRTR "input required" sentinel error to unwind the handler.
    ///
    /// The answer is stored raw, so it is deserialized into `T` -- the result
    /// type the requested kind implies -- on the replay round. A stored answer
    /// that does not fit `T` means the client answered the right key with the
    /// wrong kind of result, which is a protocol violation rather than a
    /// reason to re-ask (re-asking would loop).
    pub(crate) fn resolve<T: serde::de::DeserializeOwned>(
        &self,
        key: String,
        request: crate::types::mrtr::InputRequest,
    ) -> Result<T, Error> {
        if let Some(answer) = self.answers.get(&key) {
            return serde_json::from_value(answer.clone()).map_err(|err| {
                let reason = format!(
                    "the answer for `{key}` is not a valid {} result: {err}",
                    request.method()
                );

                if let Ok(mut malformed) = self.malformed_answer.lock() {
                    malformed.get_or_insert_with(|| reason.clone());
                }

                Error::new(ErrorCode::InvalidParams, reason)
            });
        }
        // Asking twice for one key in a round is a handler re-running its own
        // request, not two questions: keep the first and let the round carry one
        // entry per key, which is also all the map on the wire can express.
        if let Ok(mut pending) = self.pending.lock()
            && !pending.iter().any(|(pending_key, _)| *pending_key == key)
        {
            pending.push((key, request));
        }

        Err(Error::input_required())
    }

    /// Returns whether a `once` effect key has already run this chain.
    pub(crate) fn effect_seen(&self, key: &str) -> bool {
        self.effects
            .lock()
            .map(|e| e.contains(key))
            .unwrap_or(false)
    }

    /// Records a `once` effect key as run.
    pub(crate) fn record_effect(&self, key: String) {
        if let Ok(mut e) = self.effects.lock() {
            e.insert(key);
        }
    }

    /// Returns the cached `memo` value for `key`, if present.
    pub(crate) fn cached_memo(&self, key: &str) -> Option<serde_json::Value> {
        self.memos.lock().ok().and_then(|m| m.get(key).cloned())
    }

    /// Stores a `memo` value.
    pub(crate) fn store_memo(&self, key: String, value: serde_json::Value) {
        if let Ok(mut m) = self.memos.lock() {
            m.insert(key, value);
        }
    }

    /// Registers a deferred commit future.
    pub(crate) fn push_commit(&self, fut: CommitFut) {
        if let Ok(mut c) = self.commits.lock() {
            c.push(fut);
        }
    }
}

impl Context {
    /// Runs `effect` at most once across MRTR rounds (MCP 2026-07-28).
    ///
    /// On a replay (the key was recorded in a prior round) the future is
    /// dropped unpolled and `Ok(false)` is returned. On a miss the future is
    /// awaited; on success the key is recorded and `Ok(true)` is returned, on
    /// failure the error propagates and the key is **not** recorded (so the
    /// next round retries).
    ///
    /// Sync work lives inside a non-awaiting `async {}` block.
    ///
    /// # Durability
    /// The effect runs *before* the `requestState` recording it is durably
    /// acknowledged by the client -- it is at-most-once within a single
    /// `requestState` chain, **not** globally exactly-once. For non-idempotent
    /// side effects, pass a stable idempotency key to the downstream system.
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", not(feature = "legacy-spec")))] {
    /// # use neva::{Context, error::Error};
    /// # async fn f(ctx: Context) -> Result<(), Error> {
    /// ctx.once("emit_metric", async { Ok(()) }).await?;
    /// # Ok(()) }
    /// # }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub async fn once<F>(&self, key: impl Into<String>, effect: F) -> Result<bool, Error>
    where
        F: Future<Output = Result<(), Error>>,
    {
        let key = key.into();
        match &self.exec {
            ExecMode::Mrtr(mrtr) => {
                if mrtr.effect_seen(&key) {
                    return Ok(false);
                }

                effect.await?;
                mrtr.record_effect(key);

                Ok(true)
            }
            // `once` is an MRTR helper (it dedups across re-runs). A required-task
            // tool never re-runs, so using it there is a mistake -- reject it.
            #[cfg(feature = "tasks")]
            ExecMode::Task(task) if task.required => Err(Error::new(
                ErrorCode::InvalidRequest,
                "ctx.once is an MRTR helper and is not available in a required-task tool; run the effect inline",
            )),
            // Optional-task / None: there is no re-run, so the effect simply runs
            // once (the inline behavior).
            _ => {
                let _ = key;
                effect.await?;
                Ok(true)
            }
        }
    }

    /// Computes `compute` at most once across MRTR rounds and caches the
    /// serialized value in `requestState` (MCP 2026-07-28).
    ///
    /// On a replay the cached value is deserialized and returned (the future is
    /// dropped unpolled). On a miss the future is awaited, the value serialized
    /// and stored, and returned. A failed compute is not cached.
    ///
    /// Caching a value grows `requestState`; prefer [`Context::once`] when the
    /// result isn't needed later. See [`crate::App::with_max_state_bytes`].
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", not(feature = "legacy-spec")))] {
    /// # use neva::{Context, error::Error};
    /// # async fn f(ctx: Context) -> Result<(), Error> {
    /// let n: i32 = ctx.memo("answer", async { Ok(42) }).await?;
    /// # let _ = n; Ok(()) }
    /// # }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub async fn memo<T, F>(&self, key: impl Into<String>, compute: F) -> Result<T, Error>
    where
        T: serde::Serialize + serde::de::DeserializeOwned,
        F: std::future::Future<Output = Result<T, Error>>,
    {
        let key = key.into();
        match &self.exec {
            ExecMode::Mrtr(mrtr) => {
                if let Some(value) = mrtr.cached_memo(&key) {
                    return serde_json::from_value(value).map_err(Error::from);
                }

                let value = compute.await?;
                mrtr.store_memo(key, serde_json::to_value(&value).map_err(Error::from)?);

                Ok(value)
            }
            // `memo` is an MRTR helper (it caches across re-runs). A required-task
            // tool never re-runs, so using it there is a mistake -- reject it.
            #[cfg(feature = "tasks")]
            ExecMode::Task(task) if task.required => Err(Error::new(
                ErrorCode::InvalidRequest,
                "ctx.memo is an MRTR helper and is not available in a required-task tool; compute the value inline",
            )),
            // Optional-task / None: no re-run, so there is nothing to cache
            // against -- just compute the value.
            _ => {
                let _ = key;
                compute.await
            }
        }
    }

    /// Registers `effect` to run **exactly once**, when the handler reaches its
    /// final (non-`input_required`) result (MCP 2026-07-28).
    ///
    /// Commits are awaited in registration order before the final response is
    /// sent; the first `Err` becomes the response error. They do **not** run on
    /// intermediate `input_required` rounds, nor when the handler errors.
    ///
    /// Commits run whenever the tool returns a success response.
    /// If your tool encodes failure in content rather than returning `Err` or
    /// setting `isError: true`, commits will still run -- return `Err`
    /// (folded into `isError: true` by the wrapper) or set the flag explicitly
    /// to suppress them.
    ///
    /// The future is stored in the shared dispatch state, so it must be
    /// `Send + 'static` -- capture by `move`. This is an **MRTR-only** helper:
    /// a task runs on the stateful substrate and never re-runs, so in a
    /// task-augmented call `on_commit` is ignored (run the effect inline
    /// instead) -- it warns for a `Required` tool and logs at `debug` for an
    /// `Optional` one. Called outside an elicitable dispatch, it is a no-op.
    ///
    /// # Durability
    /// "Exactly once" means once per completed flow, not globally idempotent --
    /// a client that abandons and restarts the flow runs it again.
    ///
    /// A round that failed partway through its commits counts as completed for
    /// this purpose: its error is cached against that state exactly as a
    /// success would be, so retrying the same round replays the error rather
    /// than running the effects that already applied. Recovering from such a
    /// failure means starting a fresh flow, which re-runs everything.
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", not(feature = "legacy-spec")))] {
    /// # use neva::{Context, error::Error};
    /// # async fn f(ctx: Context) {
    /// ctx.on_commit(async move { Ok(()) });
    /// # }
    /// # }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub fn on_commit<F>(&self, effect: F)
    where
        F: Future<Output = Result<(), Error>> + Send + 'static,
    {
        match &self.exec {
            ExecMode::Mrtr(mrtr) => mrtr.push_commit(Box::pin(effect)),
            // `on_commit` is an MRTR helper (it defers a side effect across
            // re-runs to the final round). A task never re-runs, so the effect
            // should just be run inline; the registration is ignored here. A
            // required-task tool warns (clear mistake); an optional-task tool
            // logs at debug (it may carry `on_commit` for its bare-MRTR path).
            #[cfg(feature = "tasks")]
            ExecMode::Task(_task) =>
            {
                #[cfg(feature = "tracing")]
                if _task.required {
                    tracing::warn!(
                        logger = "neva",
                        "on_commit is an MRTR helper and is ignored in a required-task tool; run the effect inline"
                    );
                } else {
                    tracing::debug!(
                        logger = "neva",
                        "on_commit ignored in a task; run the effect inline"
                    );
                }
            }
            ExecMode::None => {
                #[cfg(feature = "tracing")]
                tracing::warn!(
                    logger = "neva",
                    "on_commit called outside an elicitable dispatch; ignored"
                );
            }
        }
    }
}

#[cfg(all(test, not(feature = "legacy-spec")))]
mod mrtr_tests {
    use super::*;
    use crate::types::elicitation::{ElicitRequestParams, ElicitResult, ElicitationAction};

    fn params() -> ElicitRequestParams {
        ElicitRequestParams::form("m")
            .with_required("x", "string")
            .into()
    }

    fn elicitation() -> crate::types::mrtr::InputRequest {
        crate::types::mrtr::InputRequest::Elicitation(params())
    }

    fn mrtr_with(answers: HashMap<String, serde_json::Value>) -> MrtrCtx {
        MrtrCtx {
            answers,
            pending: Default::default(),
            client_capabilities: crate::types::mrtr::ClientMrtrCapabilities {
                elicitation: Some(Default::default()),
                sampling: true,
                roots: true,
            },
            ..Default::default()
        }
    }

    #[test]
    fn resolve_replays_cached_answer_and_records_pending_on_miss() {
        let mut answers = HashMap::new();
        answers.insert(
            "known".to_string(),
            serde_json::to_value(ElicitResult {
                action: ElicitationAction::Accept,
                content: Some(serde_json::json!({ "x": 1 })),
                meta: None,
            })
            .unwrap(),
        );
        let mrtr = mrtr_with(answers);

        // Hit: returns the cached answer, deserialized into the kind's result.
        let got: ElicitResult = mrtr.resolve("known".into(), elicitation()).expect("cached");
        assert_eq!(got.action, ElicitationAction::Accept);

        // Miss: returns the sentinel and records pending.
        let miss = mrtr.resolve::<ElicitResult>("unknown".into(), elicitation());
        assert_eq!(miss.unwrap_err().code, ErrorCode::InputRequired);
        assert_eq!(mrtr.pending.lock().unwrap().len(), 1);
    }

    /// Every kind replays through the same slot -- only the result type differs.
    #[test]
    fn resolve_replays_each_input_kind_as_its_own_result_type() {
        use crate::types::mrtr::InputRequest;
        use crate::types::root::ListRootsResult;
        use crate::types::sampling::CreateMessageResult;

        let mut answers = std::collections::HashMap::new();
        answers.insert(
            "poem".to_string(),
            serde_json::to_value(CreateMessageResult::assistant()).unwrap(),
        );
        answers.insert(
            "dirs".to_string(),
            serde_json::json!({ "roots": [{ "uri": "file:///work", "name": "work" }] }),
        );
        let mrtr = mrtr_with(answers);

        #[allow(deprecated)]
        let sampled: CreateMessageResult = mrtr
            .resolve("poem".into(), InputRequest::Sampling(Box::default()))
            .expect("cached sampling result");
        assert_eq!(sampled.role, crate::types::Role::Assistant);

        #[allow(deprecated)]
        let roots: ListRootsResult = mrtr
            .resolve("dirs".into(), InputRequest::Roots(Default::default()))
            .expect("cached roots result");
        assert_eq!(roots.roots.len(), 1);
    }

    /// A key answered with the wrong kind of result is a protocol violation:
    /// re-asking would loop forever, so it must surface as an error.
    #[test]
    fn resolve_rejects_a_mismatched_answer_instead_of_re_asking() {
        let mut answers = std::collections::HashMap::new();
        answers.insert("k".to_string(), serde_json::json!({ "not": "an elicit" }));
        let mrtr = mrtr_with(answers);

        let err = mrtr
            .resolve::<ElicitResult>("k".into(), elicitation())
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
        assert!(
            err.to_string().contains("elicitation/create"),
            "the error must name the kind that was asked for, got: {err}"
        );
        assert!(
            mrtr.pending.lock().unwrap().is_empty(),
            "a mismatched answer must not re-request the input"
        );
    }

    #[test]
    fn effect_seen_and_record() {
        let m = MrtrCtx::default();
        assert!(!m.effect_seen("charge"));
        m.record_effect("charge".into());
        assert!(m.effect_seen("charge"));
    }

    #[test]
    fn cached_memo_store_and_fetch() {
        let m = MrtrCtx::default();
        assert!(m.cached_memo("quote").is_none());
        m.store_memo("quote".into(), serde_json::json!({"price": 42}));
        assert_eq!(
            m.cached_memo("quote"),
            Some(serde_json::json!({"price": 42}))
        );
    }

    #[test]
    fn push_commit_accumulates() {
        let m = MrtrCtx::default();
        m.push_commit(Box::pin(async { Ok(()) }));
        m.push_commit(Box::pin(async { Ok(()) }));
        assert_eq!(m.commits.lock().unwrap().len(), 2);
    }
}

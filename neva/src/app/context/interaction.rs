//! Asking the client for something mid-handler: elicitation, sampling, roots.
//!
//! Under MCP 2026-07-28 there is no server->client channel to ask on, so these
//! do not block on a reply -- they record what is wanted and unwind the round,
//! and the caller answers by re-sending the request with `inputResponses`.
//! `request_input` is that single seam; every method here is a typed front for
//! it. The legacy profile keeps the older shape, where the same calls really do
//! send a request and await its response.

use super::*;

impl Context {
    /// Requests a list of available roots from a client
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", feature = "legacy-spec"))] {
    /// use neva::{Context, error::Error, tool};
    ///
    /// #[tool]
    /// async fn handle_roots(mut ctx: Context) -> Result<(), Error> {
    ///     let roots = ctx.list_roots().await?;
    ///
    ///     // do something with roots
    ///
    /// # Ok(())
    /// }
    /// # }
    /// ```
    #[cfg(feature = "legacy-spec")]
    pub async fn list_roots(&mut self) -> Result<ListRootsResult, Error> {
        let method = crate::types::root::commands::LIST;
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(ListRootsRequestParams::default()),
        );

        self.send_request(req).await?.into_result()
    }

    /// Sends the sampling request to the client
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(feature = "server-macros")] {
    /// use neva::{
    ///     Context,
    ///     error::Error,
    ///     types::sampling::CreateMessageRequestParams,
    ///     tool
    /// };
    ///
    /// #[tool]
    /// async fn generate_poem(mut ctx: Context, topic: String) -> Result<String, Error> {
    ///     let params = CreateMessageRequestParams::new()
    ///         .with_message(format!("Write a short poem about {topic}"))
    ///         .with_sys_prompt("You are a talented poet who writes concise, evocative verses.");
    ///
    ///     let result = ctx.sample(params).await?;
    ///     Ok(format!("{:?}", result.content))
    /// }
    /// # }
    /// ```
    #[cfg(all(not(feature = "tasks"), feature = "legacy-spec"))]
    pub async fn sample(
        &mut self,
        params: CreateMessageRequestParams,
    ) -> Result<CreateMessageResult, Error> {
        let method = crate::types::sampling::commands::CREATE;
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(params),
        );

        self.send_request(req).await?.into_result()
    }

    /// Sends the sampling request to the client
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(feature = "server-macros")] {
    /// use neva::{
    ///     Context,
    ///     error::Error,
    ///     types::sampling::CreateMessageRequestParams,
    ///     tool
    /// };
    ///
    /// #[tool]
    /// async fn generate_poem(mut ctx: Context, topic: String) -> Result<String, Error> {
    ///     let params = CreateMessageRequestParams::new()
    ///         .with_message(format!("Write a short poem about {topic}"))
    ///         .with_sys_prompt("You are a talented poet who writes concise, evocative verses.");
    ///
    ///     let result = ctx.sample(params).await?;
    ///     Ok(format!("{:?}", result.content))
    /// }
    /// # }
    /// ```
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    pub async fn sample(
        &mut self,
        params: CreateMessageRequestParams,
    ) -> Result<CreateMessageResult, Error> {
        let method = crate::types::sampling::commands::CREATE;
        let is_task_aug = params.task.is_some();
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(params),
        );

        self.send_maybe_task_augmented_request(req, is_task_aug)
            .await
    }

    /// Sends the elicitation request to the client
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(feature = "serve-macros")] {
    /// use neva::{
    ///     Context,
    ///     error::Error,
    ///     types::elicitation::ElicitRequestParams,
    ///     tool
    /// };
    ///
    /// #[tool]
    /// async fn generate_poem(mut ctx: Context, _topic: String) -> Result<String, Error> {
    ///     let params = ElicitRequestParams::new("What is the poem mood you'd like?")
    ///         .with_required("mood", "string");
    ///     let result = ctx.elicit(params).await?;
    ///     Ok(format!("{:?}", result.content))
    /// }
    /// # }
    /// ```
    #[cfg(all(not(feature = "tasks"), feature = "legacy-spec"))]
    pub async fn elicit(&mut self, params: ElicitRequestParams) -> Result<ElicitResult, Error> {
        let method = crate::types::elicitation::commands::CREATE;
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(params),
        );

        self.send_request(req).await?.into_result()
    }

    /// Requests elicitation input from the client (MRTR, MCP 2026-07-28).
    ///
    /// On the first dispatch the answer for `key` is absent: the request is
    /// recorded and an internal sentinel error is returned, which the server
    /// converts into an `InputRequiredResult`. When the client retries with
    /// the answer, this handler re-runs and the call returns the cached
    /// [`ElicitResult`].
    ///
    /// **Important:** code before an `elicit` point re-executes on every
    /// round-trip -- keep it side-effect-free.
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", not(feature = "legacy-spec")))] {
    /// use neva::{Context, error::Error, types::elicitation::ElicitRequestParams, tool};
    ///
    /// #[tool]
    /// async fn greet(mut ctx: Context) -> Result<String, Error> {
    ///     let params = ElicitRequestParams::form("Your name?")
    ///         .with_required("name", "string")
    ///         .into();
    ///     let res = ctx.elicit("name", params).await?;
    ///     Ok(format!("{:?}", res.content))
    /// }
    /// # }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub async fn elicit(
        &mut self,
        key: impl Into<String>,
        params: ElicitRequestParams,
    ) -> Result<ElicitResult, Error> {
        // `ctx.elicit` is the *MRTR* (stateless re-run) entry point. A
        // task-augmented call runs on the stateful task substrate and must use
        // the explicit `ctx.task().elicit(...)` builder instead -- the two never
        // mix (see [`ExecMode`]).
        match &self.exec {
            ExecMode::Mrtr(mrtr) => mrtr.resolve(
                key.into(),
                crate::types::mrtr::InputRequest::Elicitation(params),
            ),
            #[cfg(feature = "tasks")]
            ExecMode::Task(_) => Err(Error::new(
                ErrorCode::InvalidRequest,
                "this is a task-augmented call; use ctx.task().elicit(params)",
            )),
            _ => Err(Error::new(
                ErrorCode::InvalidRequest,
                "elicitation is not available for this request",
            )),
        }
    }

    /// What the caller declared it can answer, from this request's `_meta`
    /// (MCP 2026-07-28).
    ///
    /// Capabilities are declared per request, so this is the caller of *this*
    /// call and not of some earlier handshake. Ask only for kinds it names:
    /// requesting one it did not declare is refused with
    /// [`MissingRequiredClientCapability`](crate::error::ErrorCode::MissingRequiredClientCapability),
    /// which ends the call rather than degrading it -- a handler that can do
    /// without an input should look here first and skip asking.
    ///
    /// A caller that declared nothing reads as declaring nothing, which is the
    /// same answer as a caller that cannot answer anything: either way, do not
    /// ask. Elicitation reads down to the mode -- see
    /// [`ElicitationModes`](crate::types::mrtr::ElicitationModes), whose
    /// `allows` answers "can this caller be sent *these* params".
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", not(feature = "legacy-spec")))] {
    /// use neva::{Context, error::Error, types::elicitation::ElicitRequestParams, tool};
    ///
    /// #[tool]
    /// async fn greet(mut ctx: Context) -> Result<String, Error> {
    ///     if ctx.client_capabilities().elicitation.is_none() {
    ///         return Ok("Hello, stranger!".to_string());
    ///     }
    ///
    ///     let params = ElicitRequestParams::form("Your name?")
    ///         .with_required("name", "string")
    ///         .into();
    ///
    ///     let res = ctx.elicit("name", params).await?;
    ///     Ok(format!("{:?}", res.content))
    /// }
    /// # }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub fn client_capabilities(&self) -> crate::types::mrtr::ClientMrtrCapabilities {
        self.client_capabilities
    }

    /// Requests an LLM completion from the client (MRTR, MCP 2026-07-28).
    ///
    /// Same re-run/replay semantics as [`Self::elicit`]: on the first dispatch
    /// the answer for `key` is absent, so the request is recorded and the
    /// handler unwinds; when the client retries with the result, the handler
    /// re-runs and this returns the cached [`CreateMessageResult`](crate::types::sampling::CreateMessageResult).
    ///
    /// **Important:** code before this point re-executes on every round-trip --
    /// keep it side-effect-free, or guard it with [`Self::once`] /
    /// [`Self::memo`] / [`Self::on_commit`], which work here exactly as they do
    /// for elicitation.
    ///
    /// # Deprecated on arrival
    /// MCP 2026-07-28 removed sampling as a capability-driven server->client
    /// request and re-homed the *ability* onto MRTR -- already on its
    /// deprecation path (a 12-month lifecycle shared with roots and logging).
    /// It exists for migration; prefer tools that do not need the client's
    /// model.
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", not(feature = "legacy-spec")))] {
    /// use neva::{Context, error::Error, tool};
    /// use neva::types::sampling::{CreateMessageRequestParams, SamplingMessage};
    ///
    /// #[tool]
    /// async fn summarize(mut ctx: Context, text: String) -> Result<String, Error> {
    ///     let params = CreateMessageRequestParams::new()
    ///         .with_message(SamplingMessage::user().with(format!("Summarize: {text}")));
    ///     # #[allow(deprecated)]
    ///     let res = ctx.sample("summary", params).await?;
    ///     Ok(format!("{:?}", res.content))
    /// }
    /// # }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    #[deprecated(
        note = "sampling is deprecated in MCP 2026-07-28; it returns as an MRTR input-request kind only for migration"
    )]
    pub async fn sample(
        &mut self,
        key: impl Into<String>,
        params: crate::types::sampling::CreateMessageRequestParams,
    ) -> Result<crate::types::sampling::CreateMessageResult, Error> {
        #[allow(deprecated)]
        self.request_input(
            key,
            crate::types::mrtr::InputRequest::Sampling(Box::new(params)),
            "sampling",
        )
    }

    /// Asks the client which filesystem roots it exposes (MRTR,
    /// MCP 2026-07-28).
    ///
    /// Same re-run/replay semantics as [`Self::elicit`] -- see [`Self::sample`]
    /// for the round-trip caveat.
    ///
    /// # Deprecated on arrival
    /// As with [`Self::sample`]: the capability-driven `roots/list` request is
    /// gone in MCP 2026-07-28 and the ability returns re-homed onto MRTR,
    /// already deprecated.
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", not(feature = "legacy-spec")))] {
    /// use neva::{Context, error::Error, tool};
    ///
    /// #[tool]
    /// async fn scan(mut ctx: Context) -> Result<String, Error> {
    ///     # #[allow(deprecated)]
    ///     let roots = ctx.list_roots("roots").await?;
    ///     Ok(format!("{} roots", roots.roots.len()))
    /// }
    /// # }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    #[deprecated(
        note = "roots are deprecated in MCP 2026-07-28; they return as an MRTR input-request kind only for migration"
    )]
    pub async fn list_roots(
        &mut self,
        key: impl Into<String>,
    ) -> Result<crate::types::root::ListRootsResult, Error> {
        #[allow(deprecated)]
        self.request_input(
            key,
            crate::types::mrtr::InputRequest::Roots(Default::default()),
            "roots",
        )
    }

    /// The shared body behind [`Self::elicit`] / [`Self::sample`] /
    /// [`Self::list_roots`]: every input kind rides the same MRTR substrate,
    /// so only the envelope and the result type differ.
    #[cfg(not(feature = "legacy-spec"))]
    fn request_input<T: serde::de::DeserializeOwned>(
        &self,
        key: impl Into<String>,
        request: crate::types::mrtr::InputRequest,
        kind: &str,
    ) -> Result<T, Error> {
        match &self.exec {
            ExecMode::Mrtr(mrtr) => mrtr.resolve(key.into(), request),
            #[cfg(feature = "tasks")]
            ExecMode::Task(_) => Err(Error::new(
                ErrorCode::InvalidRequest,
                format!("this is a task-augmented call; {kind} is only available on the MRTR path"),
            )),
            _ => Err(Error::new(
                ErrorCode::InvalidRequest,
                format!("{kind} is not available for this request"),
            )),
        }
    }
}

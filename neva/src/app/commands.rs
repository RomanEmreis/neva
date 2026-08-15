//! The MCP methods every server answers out of the box.
//!
//! One function per wire method, all registered in [`App::new`]. They are thin
//! by design: each unpacks its params, asks
//! [`McpOptions`](super::options::McpOptions) or [`Context`] for the answer and
//! hands back a typed result. Anything a handler has to *decide* belongs in the
//! type it decides about, not here.
//!
//! `map_handler` replaces any of them under the same method name.

use super::*;

impl App {
    /// Connection initialization handler (legacy handshake).
    #[cfg(feature = "legacy-spec")]
    pub(super) async fn init(
        options: RuntimeMcpOptions,
        _params: InitializeRequestParams,
    ) -> Result<InitializeResult, Error> {
        Ok(InitializeResult::new(&options))
    }

    /// Stateless capability discovery handler (MCP 2026-07-28).
    #[cfg(not(feature = "legacy-spec"))]
    pub(super) async fn discover(
        options: RuntimeMcpOptions,
        _params: crate::types::DiscoverRequestParams,
    ) -> Result<crate::types::DiscoverResult, Error> {
        Ok(crate::types::DiscoverResult::new(&options))
    }

    /// Completion request handler
    pub(super) async fn completion() -> CompleteResult {
        // return default as its non-optional capability so far
        CompleteResult::default()
    }

    /// Tools request handler
    #[cfg_attr(feature = "legacy-spec", allow(clippy::needless_update))]
    pub(super) async fn tools(
        options: RuntimeMcpOptions,
        params: ListToolsRequestParams,
    ) -> ListToolsResult {
        let (tools, next_cursor) = options
            .list_tools_page(params.cursor, DEFAULT_PAGE_SIZE)
            .await;

        ListToolsResult {
            tools,
            next_cursor,
            ..Default::default()
        }
    }

    /// Resources request handler
    #[cfg_attr(feature = "legacy-spec", allow(clippy::needless_update))]
    pub(super) async fn resources(
        options: RuntimeMcpOptions,
        params: ListResourcesRequestParams,
    ) -> ListResourcesResult {
        let (resources, next_cursor) = options
            .list_resources_page(params.cursor, DEFAULT_PAGE_SIZE)
            .await;

        ListResourcesResult {
            resources,
            next_cursor,
            ..Default::default()
        }
    }

    /// Resource templates request handler
    #[cfg_attr(feature = "legacy-spec", allow(clippy::needless_update))]
    pub(super) async fn resource_templates(
        options: RuntimeMcpOptions,
        params: ListResourceTemplatesRequestParams,
    ) -> ListResourceTemplatesResult {
        let (resource_templates, next_cursor) = options
            .list_resource_templates_page(params.cursor, DEFAULT_PAGE_SIZE)
            .await;

        ListResourceTemplatesResult {
            templates: resource_templates,
            next_cursor,
            ..Default::default()
        }
    }

    /// Prompts request handler
    #[cfg_attr(feature = "legacy-spec", allow(clippy::needless_update))]
    pub(super) async fn prompts(
        options: RuntimeMcpOptions,
        params: ListPromptsRequestParams,
    ) -> ListPromptsResult {
        let (prompts, next_cursor) = options
            .list_prompts_page(params.cursor, DEFAULT_PAGE_SIZE)
            .await;

        ListPromptsResult {
            prompts,
            next_cursor,
            ..Default::default()
        }
    }

    /// A tool call request handler
    #[cfg(not(feature = "tasks"))]
    pub(super) async fn tool(
        ctx: Context,
        params: CallToolRequestParams,
    ) -> Result<CallToolResponse, Error> {
        ctx.call_tool(params).await
    }

    /// A tool call request handler
    #[cfg(feature = "tasks")]
    pub(super) async fn tool(
        ctx: Context,
        params: CallToolRequestParams,
    ) -> Result<ToolOrTaskResponse, Error> {
        ctx.call_tool_with_task(params).await
    }

    /// A read resource request handler
    pub(super) async fn resource(
        ctx: Context,
        params: ReadResourceRequestParams,
    ) -> Result<ReadResourceResult, Error> {
        ctx.read_resource(params).await
    }

    /// A get prompt request handler
    pub(super) async fn prompt(
        ctx: Context,
        params: GetPromptRequestParams,
    ) -> Result<GetPromptResult, Error> {
        ctx.get_prompt(params).await
    }

    /// Ping request handler
    #[cfg(feature = "legacy-spec")]
    pub(super) async fn ping() {}

    /// A subscription to a resource change request handler
    ///
    /// Not registered under MCP 2026-07-28, where the method is folded into the
    /// `subscriptions/listen` filter; see [`Self::subscriptions_listen`].
    #[cfg(feature = "legacy-spec")]
    pub(super) async fn resource_subscribe(mut ctx: Context, params: SubscribeRequestParams) {
        ctx.subscribe_to_resource(params.uri);
    }

    /// An unsubscription to from resource change request handler
    ///
    /// Not registered under MCP 2026-07-28; see [`Self::resource_subscribe`].
    #[cfg(feature = "legacy-spec")]
    pub(super) async fn resource_unsubscribe(mut ctx: Context, params: UnsubscribeRequestParams) {
        ctx.unsubscribe_from_resource(&params.uri);
    }

    /// A `subscriptions/listen` request handler (MCP 2026-07-28).
    ///
    /// The request stays open for the life of the subscription: the accepted
    /// filter is acknowledged first, notifications matching it flow on the same
    /// stream, and the reply -- an empty result carrying the subscription id --
    /// is what marks a graceful close.
    #[cfg(not(feature = "legacy-spec"))]
    pub(super) async fn subscriptions_listen(
        ctx: Context,
        id: RequestId,
        params: SubscriptionsListenRequestParams,
    ) -> Result<SubscriptionsListenResult, Error> {
        ctx.listen(id, params.notifications).await
    }

    /// Tasks request handler
    ///
    /// Not registered under MCP 2026-07-28: the final Tasks extension has no
    /// `tasks/list`. A task id is a durable handle the requestor already holds.
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    pub(super) async fn tasks(
        options: RuntimeMcpOptions,
        params: ListTasksRequestParams,
    ) -> Result<ListTasksResult, Error> {
        if !options.is_tasks_list_supported() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "Server does not support support tasks/list requests.",
            ));
        }

        Ok(options
            .list_tasks()
            .paginate(params.cursor, DEFAULT_PAGE_SIZE)
            .into())
    }

    /// A cancel task request handler
    ///
    /// Under MCP 2026-07-28 cancellation is cooperative and the reply is an
    /// empty acknowledgement -- the requestor learns the outcome by polling
    /// `tasks/get`, since the task may still reach a non-`cancelled` terminal
    /// status. `legacy-spec` returns the cancelled [`Task`] instead.
    #[cfg(all(feature = "tasks", not(feature = "legacy-spec")))]
    pub(super) async fn cancel_task(
        options: RuntimeMcpOptions,
        params: CancelTaskRequestParams,
    ) -> Result<(), Error> {
        options.cancel_task(&params.id).map(|_| ())
    }

    /// A cancel task request handler
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    pub(super) async fn cancel_task(
        options: RuntimeMcpOptions,
        params: CancelTaskRequestParams,
    ) -> Result<Task, Error> {
        if options.is_tasks_cancellation_supported() {
            options.cancel_task(&params.id)
        } else {
            Err(Error::new(
                ErrorCode::InvalidRequest,
                "Server does not support support tasks/cancel requests.",
            ))
        }
    }

    /// A task state retrieval request handler
    ///
    /// Under MCP 2026-07-28 this is the single polling method: the reply
    /// carries the status plus, depending on it, the outstanding
    /// `inputRequests`, the terminal `result`, or the `error`.
    #[cfg(all(feature = "tasks", not(feature = "legacy-spec")))]
    pub(super) async fn task(
        options: RuntimeMcpOptions,
        params: GetTaskRequestParams,
    ) -> Result<DetailedTask, Error> {
        options.get_task_state(&params.id)
    }

    /// A task status retrieval request handler
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    pub(super) async fn task(
        options: RuntimeMcpOptions,
        params: GetTaskRequestParams,
    ) -> Result<Task, Error> {
        options.get_task_status(&params.id)
    }

    /// A task input submission request handler (MCP 2026-07-28)
    ///
    /// Answers the task's outstanding input requests and acknowledges with an
    /// empty result. Responses for unknown or already-satisfied keys are
    /// ignored, per the spec.
    #[cfg(all(feature = "tasks", not(feature = "legacy-spec")))]
    pub(super) async fn update_task(
        options: RuntimeMcpOptions,
        params: UpdateTaskRequestParams,
    ) -> Result<(), Error> {
        options.update_task(&params.id, params.input_responses)
    }

    /// A task result retrieval request handler
    ///
    /// Not registered under MCP 2026-07-28: result retrieval folded into
    /// `tasks/get`.
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    pub(super) async fn task_result(
        options: RuntimeMcpOptions,
        params: GetTaskPayloadRequestParams,
    ) -> Result<TaskPayload, Error> {
        options.get_task_result(&params.id).await
    }

    /// Sets the logging level
    #[allow(deprecated)]
    #[cfg(all(feature = "tracing", feature = "legacy-spec"))]
    pub(super) async fn set_log_level(
        options: RuntimeMcpOptions,
        params: SetLevelRequestParams,
    ) -> Result<(), Error> {
        let current_level = options.log_level();
        tracing::debug!(
            logger = "neva",
            "Logging level has been changed from {:?} to {:?}",
            current_level,
            params.level
        );

        options.set_log_level(params.level)
    }
}

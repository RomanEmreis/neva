//! The task-augmented substrate: a tool that runs as a background task.
//!
//! Deliberately *not* MRTR. A task-augmented call runs once, in a live future
//! that genuinely suspends, with the task tracker holding its state -- so it
//! has no `requestState`, no replay log and no re-runs, and the MRTR effect
//! helpers do not apply to it. [`TaskContext`] is the explicit way in
//! (`ctx.task()`), which is what keeps the two substrates from being confused
//! for one another.

use super::*;

/// Per-dispatch state for a background, task-augmented tool call
/// (MCP 2026-07-28 + `tasks`): just the task id (also the
/// session-independent resume key) and whether the tool's task support is
/// `Required`. No MRTR key, `requestState`, or replay log -- tasks run on the
/// stateful substrate, not MRTR, so the MRTR effect helpers do not apply here.
#[cfg(all(not(feature = "legacy-spec"), feature = "tasks"))]
#[derive(Default)]
pub(crate) struct TaskExec {
    /// The server-generated task id (also the session-independent resume key).
    pub(crate) id: String,
    /// Whether the tool declared `TaskSupport::Required`. A required-task tool is
    /// *only* ever a task, so calling an MRTR helper there is a clear mistake and
    /// is rejected; an optional-task tool may carry MRTR helpers for its bare
    /// path, so they degrade quietly when it happens to run as a task.
    pub(crate) required: bool,
}

#[cfg(all(not(feature = "legacy-spec"), feature = "tasks"))]
impl TaskExec {
    /// Creates a task execution context for `id`.
    pub(crate) fn new(id: String, required: bool) -> Self {
        Self { id, required }
    }
}

/// Task-scoped API for a task-augmented call (MCP 2026-07-28 + `tasks`).
///
/// Obtained via [`Context::task`]; mirrors the client's `Client::task()` builder.
/// Its methods operate on the stateful task substrate (suspend/resume) and error
/// when the current dispatch is not task-augmented -- keeping the task and MRTR
/// substrates explicitly separate.
#[cfg(all(not(feature = "legacy-spec"), feature = "tasks"))]
#[derive(Debug)]
pub struct TaskContext<'a> {
    ctx: &'a mut Context,
}

#[cfg(all(not(feature = "legacy-spec"), feature = "tasks"))]
impl TaskContext<'_> {
    /// Requests input from the client and suspends the background task until the
    /// answer arrives.
    ///
    /// Unlike the MRTR [`Context::elicit`], this takes no replay `key`: a task
    /// does not re-run, it genuinely awaits. Errors when the current dispatch is
    /// not a task-augmented call (use [`Context::elicit`] there instead).
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", not(feature = "legacy-spec"), feature = "tasks"))] {
    /// # use neva::{Context, error::Error, types::elicitation::ElicitRequestParams};
    ///
    /// # async fn f(mut ctx: Context, params: ElicitRequestParams) -> Result<(), Error> {
    /// let _ans = ctx.task().elicit(params).await?;
    /// # Ok(()) }
    /// # }
    /// ```
    pub async fn elicit(self, params: ElicitRequestParams) -> Result<ElicitResult, Error> {
        let task_id = match &self.ctx.exec {
            ExecMode::Task(task) => task.id.clone(),
            _ => {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "not a task-augmented call; use ctx.elicit(key, params)",
                ));
            }
        };
        self.ctx.task_elicit(task_id, params).await
    }
}

impl Context {
    #[inline]
    #[cfg(feature = "tasks")]
    pub(crate) async fn call_tool_with_task(
        self,
        params: CallToolRequestParams,
    ) -> Result<ToolOrTaskResponse, Error> {
        match self.options.get_tool(&params.name).await {
            None => Err(Error::new(ErrorCode::InvalidParams, "Tool not found")),
            Some(tool) => {
                #[cfg(feature = "http-server")]
                self.validate_claims(tool.roles.as_deref(), tool.permissions.as_deref())?;

                let task_support = tool.task_support();
                if let Some(task_meta) = params.task {
                    self.ensure_tool_augmentation_support(task_support)?;

                    let task = Task::from(task_meta);
                    let handle = self.options.track_task(task.clone());

                    let opt = self.options.clone();
                    let task_id = task.id.clone();

                    // The tool runs in this spawned task on the *stateful* task
                    // substrate -- not MRTR. Under MCP 2026-07-28 it gets a `Task`
                    // execution context (no MRTR key / `requestState`): elicitation
                    // goes through `ctx.task().elicit(...)`, which suspends on the
                    // task tracker (resumed by a client answer keyed by the task
                    // id). The MRTR effect helpers (`once`/`memo`/`on_commit`) do
                    // not apply on this substrate (see their docs).
                    #[cfg(all(not(feature = "legacy-spec"), feature = "tasks"))]
                    let required = task_support.is_some_and(|ts| ts == TaskSupport::Required);
                    #[cfg(all(not(feature = "legacy-spec"), feature = "tasks"))]
                    let ctx = Context {
                        exec: ExecMode::Task(std::sync::Arc::new(TaskExec::new(
                            task_id.clone(),
                            required,
                        ))),
                        ..self
                    };
                    #[cfg(not(all(not(feature = "legacy-spec"), feature = "tasks")))]
                    let ctx = self;

                    tokio::spawn(async move {
                        tokio::select! {
                            result = tool.call(params
                                .with_task(&task_id)
                                .with_context(ctx)) => {
                                // The outcome is stored *before* the status
                                // flips, so a `tasks/get` that observes a
                                // terminal status always sees the matching
                                // `result` / `error` with it.
                                #[cfg(not(feature = "legacy-spec"))]
                                match result {
                                    Ok(result) => {
                                        opt.tasks.set_outcome(
                                            &task_id,
                                            serde_json::to_value(result).map_err(Error::from),
                                        );
                                        opt.tasks.complete(&task_id);
                                    },
                                    Err(err) => {
                                        opt.tasks.set_outcome(&task_id, Err(err));
                                        opt.tasks.fail(&task_id);
                                    }
                                }
                                #[cfg(feature = "legacy-spec")]
                                {
                                    let resp = match result {
                                        Ok(result) => {
                                            opt.tasks.complete(&task_id);
                                            result
                                        },
                                        Err(err) => {
                                            opt.tasks.fail(&task_id);
                                            CallToolResponse::error(err)
                                        }
                                    };
                                    handle.set_result(resp);
                                }
                            },
                            _ = handle.cancelled() => {}
                        }
                    });

                    Ok(Either::Left(CreateTaskResult::new(task)))
                } else if task_support.is_some_and(|ts| ts == TaskSupport::Required) {
                    Err(Error::new(
                        ErrorCode::MethodNotFound,
                        "Tool required task augmented call",
                    ))
                } else {
                    tool.call(params.with_context(self))
                        .await
                        .map(Either::Right)
                }
            }
        }
    }

    /// Returns whether the current dispatch is a task-augmented call
    /// (MCP 2026-07-28).
    ///
    /// Use this to branch in a `TaskSupport::Optional` tool that wants to elicit
    /// on both substrates: `ctx.task().elicit(params)` when `true`, the MRTR
    /// `ctx.elicit(key, params)` otherwise.
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", not(feature = "legacy-spec"), feature = "tasks"))] {
    /// # use neva::{Context, error::Error, types::elicitation::ElicitRequestParams};
    /// # async fn f(mut ctx: Context, params: ElicitRequestParams) -> Result<(), Error> {
    /// let _ans = if ctx.is_task() {
    ///     ctx.task().elicit(params).await?
    /// } else {
    ///     ctx.elicit("name", params).await?
    /// };
    /// # Ok(()) }
    /// # }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub fn is_task(&self) -> bool {
        #[cfg(feature = "tasks")]
        {
            matches!(self.exec, ExecMode::Task(_))
        }
        #[cfg(not(feature = "tasks"))]
        {
            false
        }
    }

    /// Returns the task-scoped API for a task-augmented call
    /// (MCP 2026-07-28 + `tasks`).
    ///
    /// Mirrors the client's `Client::task()` builder. Its methods operate on the
    /// stateful task substrate and error when the current dispatch is *not*
    /// task-augmented (check with [`Context::is_task`]).
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", not(feature = "legacy-spec"), feature = "tasks"))] {
    /// # use neva::{Context, error::Error, types::elicitation::ElicitRequestParams};
    /// # async fn f(mut ctx: Context, params: ElicitRequestParams) -> Result<(), Error> {
    /// let _ans = ctx.task().elicit(params).await?;
    /// # Ok(()) }
    /// # }
    /// ```
    #[cfg(all(not(feature = "legacy-spec"), feature = "tasks"))]
    pub fn task(&mut self) -> TaskContext<'_> {
        TaskContext { ctx: self }
    }

    /// Suspends a task-augmented elicit until the client posts an answer.
    ///
    /// Records the ask as an outstanding input request on the task and flips it
    /// to `input_required`, so the next `tasks/get` surfaces it under
    /// `inputRequests`. The live background future then awaits the answer,
    /// which arrives as a `tasks/update` addressed to this task id and keyed by
    /// the same key.
    ///
    /// The key is server-assigned and must be unique over the task's lifetime,
    /// per the spec; it stays outstanding until answered, so a retried
    /// `tasks/update` carrying it still matches.
    #[cfg(all(not(feature = "legacy-spec"), feature = "tasks"))]
    async fn task_elicit(
        &mut self,
        task_id: String,
        params: ElicitRequestParams,
    ) -> Result<ElicitResult, Error> {
        let key = uuid::Uuid::new_v4().to_string();
        let receiver = self
            .options
            .tasks
            .park_input(
                &task_id,
                key,
                crate::types::mrtr::InputRequest::Elicitation(params),
            )
            .ok_or_else(|| {
                Error::new(ErrorCode::InternalError, "task not found for elicitation")
            })?;
        self.options.tasks.require_input(&task_id);

        let answer = match timeout(self.timeout, receiver).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(_)) => {
                self.options.tasks.fail(&task_id);
                return Err(Error::new(
                    ErrorCode::InternalError,
                    "elicitation channel closed",
                ));
            }
            Err(_) => {
                self.options.tasks.fail(&task_id);
                return Err(Error::new(ErrorCode::Timeout, "Request timed out"));
            }
        };

        // `provide_inputs` already reset the task to `working`.
        serde_json::from_value(answer).map_err(Error::from)
    }
}

// The server as requestor against a client-hosted task. MCP 2026-07-28 has no
// server->client requests at all, so this whole direction is legacy-only.
#[cfg(all(feature = "tasks", feature = "legacy-spec"))]
impl crate::shared::TaskApi for Context {
    /// Retrieve task result from the client. If the task is not completed yet, waits until it completes or cancels.
    async fn get_task_result<T>(&mut self, id: impl Into<String>) -> Result<T, Error>
    where
        T: DeserializeOwned,
    {
        let params = GetTaskPayloadRequestParams { id: id.into() };
        let method = crate::types::task::commands::RESULT;
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(params),
        );

        self.send_request(req).await?.into_result()
    }

    /// Retrieve task status from the client
    async fn get_task(&mut self, id: impl Into<String>) -> Result<Task, Error> {
        let params = GetTaskRequestParams { id: id.into() };
        let method = crate::types::task::commands::GET;
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(params),
        );

        self.send_request(req).await?.into_result()
    }

    /// Cancels a task that is currently running on the client
    async fn cancel_task(&mut self, id: impl Into<String>) -> Result<Task, Error> {
        if !self.options.is_tasks_cancellation_supported() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "Server does not support cancelling tasks.",
            ));
        }

        let params = CancelTaskRequestParams { id: id.into() };
        let method = crate::types::task::commands::CANCEL;
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(params),
        );

        self.send_request(req).await?.into_result()
    }

    /// Retrieves a list of tasks from the client
    async fn list_tasks(&mut self, cursor: Option<Cursor>) -> Result<ListTasksResult, Error> {
        if !self.options.is_tasks_list_supported() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "Server does not support retrieving a task list.",
            ));
        }

        let params = ListTasksRequestParams { cursor };
        let method = crate::types::task::commands::LIST;
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(params),
        );

        self.send_request(req).await?.into_result()
    }

    async fn handle_input(&mut self, _id: &str, _params: TaskPayload) -> Result<(), Error> {
        // Reserved, there are no cases so far, for the server
        // to handle input requests from client.
        Ok(())
    }
}

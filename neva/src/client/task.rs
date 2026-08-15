//! Fluent task-augmented request builder for the MCP client

use super::Client;
#[cfg(all(feature = "tasks", feature = "legacy-spec"))]
use crate::types::{
    CancelTaskRequestParams, Cursor, GetTaskPayloadRequestParams, GetTaskRequestParams,
    ListTasksRequestParams, ListTasksResult, RequestId, Task, TaskPayload,
    elicitation::ElicitRequestParams,
};
#[cfg(all(feature = "tasks", not(feature = "legacy-spec")))]
use crate::types::{
    CancelTaskRequestParams, DetailedTask, GetTaskRequestParams, UpdateTaskRequestParams,
};
use crate::{
    error::{Error, ErrorCode},
    shared::{self, IntoArgs},
    types::{CallToolRequestParams, CallToolResponse, TaskMetadata},
};
#[cfg(all(feature = "tasks", feature = "legacy-spec"))]
use serde::de::DeserializeOwned;

/// A fluent builder for constructing and sending a task-augmented `tools/call` request.
///
/// Obtain via [`Client::task`]. Configure task options with the provided setters,
/// then call [`TaskBuilder::call_tool`] to execute.
///
/// # Example
/// ```no_run
/// use neva::client::Client;
/// use neva::error::Error;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Error> {
///     let mut client = Client::new();
///     client.connect().await?;
///
///     let result = client
///         .task()
///         .with_ttl(5000)
///         .call_tool("echo", [("message", "Hello MCP!")])
///         .await?;
///
///     println!("{result:?}");
///     client.disconnect().await
/// }
/// ```
pub struct TaskBuilder<'a> {
    pub(super) client: &'a mut Client,
    pub(super) metadata: TaskMetadata,
}

impl std::fmt::Debug for TaskBuilder<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskBuilder")
            .field("metadata", &self.metadata)
            .finish_non_exhaustive()
    }
}

impl<'a> TaskBuilder<'a> {
    /// Sets the time-to-live (in milliseconds) for the task.
    ///
    /// This requests the server to retain the task for at most `ttl` milliseconds
    /// after creation.
    pub fn with_ttl(mut self, ttl: usize) -> Self {
        self.metadata.ttl = Some(ttl);
        self
    }

    /// Sends a task-augmented `tools/call` request and waits for the task to complete.
    ///
    /// # Errors
    /// Returns [`Error`] if the server does not support task-augmented tool calls,
    /// or if the underlying request fails.
    pub async fn call_tool<N, Args>(self, name: N, args: Args) -> Result<CallToolResponse, Error>
    where
        N: Into<String>,
        Args: IntoArgs,
    {
        // Distinguished from a plain "no support" so the cause is actionable:
        // the peer may well support tasks -- just not in a dialect this build
        // speaks.
        #[cfg(not(feature = "legacy-spec"))]
        if self.client.is_legacy_peer() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "The peer negotiated the legacy protocol through the dual-mode \
                 fallback, and its task wire shape is not compiled into this \
                 build. Enable the `legacy-spec` feature to run task-augmented \
                 requests against a legacy server.",
            ));
        }

        if !self.client.is_server_support_call_tool_with_tasks() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "Server does not support call tool with tasks.",
            ));
        }

        let params = CallToolRequestParams {
            name: name.into(),
            meta: None,
            args: args.into_args(),
            task: Some(self.metadata),
        };

        let result = self.client.call_tool_raw(params).await?.into_result()?;
        shared::wait_to_completion(self.client, result).await
    }
}

#[cfg(all(feature = "tasks", not(feature = "legacy-spec")))]
impl shared::TaskApi for Client {
    /// Retrieves the full task state: status plus, depending on it, the
    /// outstanding input requests, the terminal result, or the error.
    async fn get_task(&mut self, id: impl Into<String>) -> Result<DetailedTask, Error> {
        let params = GetTaskRequestParams { id: id.into() };
        self.command(crate::types::task::commands::GET, Some(params))
            .await?
            .into_result()
    }

    /// Submits responses to a task's outstanding input requests.
    async fn update_task(
        &mut self,
        id: impl Into<String>,
        responses: crate::types::mrtr::InputResponses,
    ) -> Result<(), Error> {
        let params = UpdateTaskRequestParams {
            id: id.into(),
            input_responses: responses,
        };

        self.command(crate::types::task::commands::UPDATE, Some(params))
            .await
            .map(|_| ())
    }

    /// Cancels a task that is currently running.
    ///
    /// The reply is an empty acknowledgement: cancellation is cooperative, so
    /// the task may still reach a non-`cancelled` terminal status. Poll
    /// `get_task` to learn the outcome.
    async fn cancel_task(&mut self, id: impl Into<String>) -> Result<(), Error> {
        let params = CancelTaskRequestParams { id: id.into() };
        self.command(crate::types::task::commands::CANCEL, Some(params))
            .await
            .map(|_| ())
    }

    /// Answers one outstanding input request with the client's configured
    /// handler for that kind.
    async fn fulfil_input(
        &mut self,
        request: &crate::types::mrtr::InputRequest,
    ) -> Result<serde_json::Value, Error> {
        use crate::types::mrtr::InputRequest;

        match request {
            InputRequest::Elicitation(params) => {
                let handler = self.options.elicitation_handler.as_ref().ok_or_else(|| {
                    Error::new(
                        ErrorCode::InvalidRequest,
                        "Client has no elicitation handler. Configure one with `Client::map_elicitation(...)`.",
                    )
                })?;
                let result = handler(params.clone()).await;
                serde_json::to_value(result).map_err(Into::into)
            }
            other => Err(Error::new(
                ErrorCode::InvalidRequest,
                format!(
                    "Client cannot fulfil `{}` input requests on the task substrate",
                    other.method()
                ),
            )),
        }
    }
}

#[cfg(all(feature = "tasks", feature = "legacy-spec"))]
impl shared::TaskApi for Client {
    /// Retrieves task result. If the task is not completed yet, waits until it completes or cancels.
    async fn get_task_result<T>(&mut self, id: impl Into<String>) -> Result<T, Error>
    where
        T: DeserializeOwned,
    {
        let params = GetTaskPayloadRequestParams { id: id.into() };
        self.command(crate::types::task::commands::RESULT, Some(params))
            .await?
            .into_result()
    }

    /// Retrieve task status
    async fn get_task(&mut self, id: impl Into<String>) -> Result<Task, Error> {
        let params = GetTaskRequestParams { id: id.into() };
        self.command(crate::types::task::commands::GET, Some(params))
            .await?
            .into_result()
    }

    /// Cancels a task that is currently running
    ///
    /// # Panics
    /// If the client or server does not support cancelling tasks
    async fn cancel_task(&mut self, id: impl Into<String>) -> Result<Task, Error> {
        assert!(
            self.is_client_support_cancelling_tasks(),
            "Client does not support cancelling tasks.  You may configure it with `Client::with_options(|opt| opt.with_tasks(...))` method."
        );

        assert!(
            self.is_server_support_cancelling_tasks(),
            "Server does not support cancelling tasks."
        );

        let params = CancelTaskRequestParams { id: id.into() };
        self.command(crate::types::task::commands::CANCEL, Some(params))
            .await?
            .into_result()
    }

    /// Retrieves a list of tasks
    ///
    /// # Panics
    /// If the client or server does not support retrieving a task list
    async fn list_tasks(&mut self, cursor: Option<Cursor>) -> Result<ListTasksResult, Error> {
        assert!(
            self.is_client_support_task_list(),
            "Client does not support retrieving a task list.  You may configure it with `Client::with_options(|opt| opt.with_tasks(...))` method."
        );

        assert!(
            self.is_server_support_task_list(),
            "Server does not support retrieving a task list."
        );

        let params = ListTasksRequestParams { cursor };
        self.command(crate::types::task::commands::LIST, Some(params))
            .await?
            .into_result()
    }

    async fn handle_input(&mut self, id: &str, params: TaskPayload) -> Result<(), Error> {
        let params = params.to::<ElicitRequestParams>()?;
        if let Some(handler) = &self.options.elicitation_handler {
            use crate::types::IntoResponse;

            let result = handler(params).await.with_related_task(id);

            let id = id.parse::<RequestId>().expect("Invalid Request Id");

            self.send_response(result.into_response(id)).await?;
        }
        Ok(())
    }
}

//! Fluent task-augmented request builder for the MCP client

use super::Client;
use crate::{
    error::{Error, ErrorCode},
    shared::{self, IntoArgs},
    types::{CallToolRequestParams, CallToolResponse, TaskMetadata},
};

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

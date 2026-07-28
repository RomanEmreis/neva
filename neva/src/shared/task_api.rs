//! Utilities and types for handling tasks

use super::Either;
use crate::error::{Error, ErrorCode};
use crate::types::{CreateTaskResult, TaskStatus};
use serde::de::DeserializeOwned;
use std::time::Duration;

#[cfg(feature = "legacy-spec")]
use crate::types::{Cursor, ListTasksResult, Task, TaskPayload};

#[cfg(not(feature = "legacy-spec"))]
use crate::types::{
    DetailedTask,
    mrtr::{InputRequest, InputResponses},
};

const DEFAULT_POLL_INTERVAL: usize = 5000; // 5 seconds

/// A trait for requestor types
pub trait TaskApi {
    /// Retrieve task result from the client. If the task is not completed yet, waits until it completes or cancels.
    #[cfg(feature = "legacy-spec")]
    fn get_task_result<T: DeserializeOwned>(
        &mut self,
        id: impl Into<String>,
    ) -> impl Future<Output = Result<T, Error>>;

    /// Retrieve task status from the client
    #[cfg(feature = "legacy-spec")]
    fn get_task(&mut self, id: impl Into<String>) -> impl Future<Output = Result<Task, Error>>;

    /// Retrieves the full task state (`tasks/get`): the status plus, depending
    /// on it, the outstanding input requests, the terminal result, or the error.
    #[cfg(not(feature = "legacy-spec"))]
    fn get_task(
        &mut self,
        id: impl Into<String>,
    ) -> impl Future<Output = Result<DetailedTask, Error>>;

    /// Submits responses to a task's outstanding input requests
    /// (`tasks/update`).
    #[cfg(not(feature = "legacy-spec"))]
    fn update_task(
        &mut self,
        id: impl Into<String>,
        responses: InputResponses,
    ) -> impl Future<Output = Result<(), Error>>;

    /// Cancels a task that is currently running on the client
    ///
    /// Cancellation is cooperative: the acknowledgement means the intent was
    /// received, not that the task stopped.
    #[cfg(not(feature = "legacy-spec"))]
    fn cancel_task(&mut self, id: impl Into<String>) -> impl Future<Output = Result<(), Error>>;

    /// Cancels a task that is currently running on the client
    #[cfg(feature = "legacy-spec")]
    fn cancel_task(&mut self, id: impl Into<String>) -> impl Future<Output = Result<Task, Error>>;

    /// Retrieves a list of tasks from the client
    ///
    /// Removed in MCP 2026-07-28: the final Tasks extension has no
    /// `tasks/list`.
    #[cfg(feature = "legacy-spec")]
    fn list_tasks(
        &mut self,
        cursor: Option<Cursor>,
    ) -> impl Future<Output = Result<ListTasksResult, Error>>;

    /// Input callback
    #[cfg(feature = "legacy-spec")]
    fn handle_input(
        &mut self,
        id: &str,
        params: TaskPayload,
    ) -> impl Future<Output = Result<(), Error>>;

    /// Fulfils one of a task's outstanding input requests, returning the raw
    /// result the peer expects back under the same key.
    #[cfg(not(feature = "legacy-spec"))]
    fn fulfil_input(
        &mut self,
        request: &InputRequest,
    ) -> impl Future<Output = Result<serde_json::Value, Error>>;
}

/// Polls the receiver with `tasks/get` until the task reaches a terminal state,
/// answering any input requests it surfaces along the way with `tasks/update`.
///
/// A `completed` task's `result` is deserialized into `T`; a `failed` task's
/// `error` is returned as an [`Error`]. A task whose TTL elapses is cancelled.
#[cfg(not(feature = "legacy-spec"))]
pub async fn wait_to_completion<A, T>(
    api: &mut A,
    result: Either<CreateTaskResult, T>,
) -> Result<T, Error>
where
    A: TaskApi,
    T: DeserializeOwned,
{
    let task_id = match result {
        Either::Right(result) => return Ok(result),
        Either::Left(task_result) => task_result.task.id,
    };

    // The server retains a task for `ttlMs` from *its* `createdAt` and drops it
    // afterwards, so the wait has to be measured against the same wall clock or
    // it outlives what it is waiting for. Measured locally and monotonically
    // rather than as `now - createdAt`: `createdAt` is the server's clock, and
    // a client running a few minutes ahead would otherwise declare every task
    // expired on the first poll. The cost is starting the count a round-trip
    // late, which errs toward waiting slightly longer than the server does.
    let waiting_since = tokio::time::Instant::now();

    loop {
        let task = api.get_task(&task_id).await?;

        // `ttlMs` may change over a task's lifetime, so it is re-read every
        // poll. Terminal statuses are answered below whatever the clock says:
        // a result that arrived is a result, even if it arrived late.
        if matches!(task.status, TaskStatus::Working | TaskStatus::InputRequired)
            && task
                .ttl
                .is_some_and(|ttl| waiting_since.elapsed().as_millis() >= ttl as u128)
        {
            #[cfg(feature = "tracing")]
            tracing::trace!(logger = "neva", "Task TTL expired. Cancelling task.");

            // Best-effort: the server may already have dropped the task, and
            // that failure must not mask why the wait ended.
            let _ = api.cancel_task(&task_id).await;
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "Task was cancelled: TTL expired",
            ));
        }

        match task.status {
            TaskStatus::Completed => {
                let result = task.result.ok_or_else(|| {
                    Error::new(ErrorCode::InternalError, "Completed task carried no result")
                })?;
                return serde_json::from_value(result).map_err(Into::into);
            }
            TaskStatus::Failed => {
                return Err(task
                    .error
                    .and_then(|err| serde_json::from_value::<crate::types::ErrorDetails>(err).ok())
                    .map_or_else(
                        || Error::new(ErrorCode::InternalError, "Task failed"),
                        Into::into,
                    ));
            }
            TaskStatus::Cancelled => {
                return Err(Error::new(ErrorCode::InvalidRequest, "Task was cancelled"));
            }
            TaskStatus::InputRequired => {
                #[cfg(feature = "tracing")]
                tracing::trace!(logger = "neva", "Task input required. Providing input.");

                let requests = task.input_requests.unwrap_or_default();
                let mut responses = InputResponses::with_capacity(requests.len());
                for (key, request) in &requests {
                    responses.insert(key.clone(), api.fulfil_input(request).await?);
                }
                api.update_task(&task_id, responses).await?;
            }
            TaskStatus::Working => {
                let poll_interval = task.poll_interval.unwrap_or(DEFAULT_POLL_INTERVAL);

                #[cfg(feature = "tracing")]
                tracing::trace!(
                    logger = "neva",
                    "Waiting for task to complete. Elapsed: {}ms",
                    waiting_since.elapsed().as_millis()
                );

                tokio::time::sleep(Duration::from_millis(poll_interval as u64)).await;
            }
        }
    }
}

/// Polls receiver with `tasks/get` until it completed, failed, cancelled or expired.
/// Call `tasks/result` if it completed or failed and `tasks/cancel` if expired.
#[cfg(feature = "legacy-spec")]
pub async fn wait_to_completion<A, T>(
    api: &mut A,
    result: Either<CreateTaskResult, T>,
) -> Result<T, Error>
where
    A: TaskApi,
    T: DeserializeOwned,
{
    let mut task = match result {
        Either::Right(result) => return Ok(result),
        Either::Left(task_result) => task_result.task,
    };

    let mut elapsed = 0;

    loop {
        if task.ttl.is_some_and(|ttl| ttl <= elapsed) {
            #[cfg(feature = "tracing")]
            tracing::trace!(logger = "neva", "Task TTL expired. Cancelling task.");

            let _ = api.cancel_task(&task.id).await?;
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "Task was cancelled: TTL expired",
            ));
        }

        task = api.get_task(&task.id).await?;

        match task.status {
            TaskStatus::Completed | TaskStatus::Failed => {
                return api.get_task_result(&task.id).await;
            }
            TaskStatus::Cancelled => {
                return Err(Error::new(ErrorCode::InvalidRequest, "Task was cancelled"));
            }
            TaskStatus::InputRequired => {
                #[cfg(feature = "tracing")]
                tracing::trace!(logger = "neva", "Task input required. Providing input.");

                let params: TaskPayload = api.get_task_result(&task.id).await?;
                api.handle_input(&task.id, params).await?;
            }
            _ => {
                let poll_interval = task.poll_interval.unwrap_or(DEFAULT_POLL_INTERVAL);

                elapsed += poll_interval;

                #[cfg(feature = "tracing")]
                tracing::trace!(
                    logger = "neva",
                    "Waiting for task to complete. Elapsed: {elapsed}ms"
                );

                tokio::time::sleep(Duration::from_millis(poll_interval as u64)).await;
            }
        }
    }
}

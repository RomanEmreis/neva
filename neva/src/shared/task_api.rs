//! Utilities and types for handling tasks

use crate::types::{Task, TaskPayload, TaskStatus, CreateTaskResult, ListTasksResult, Cursor};
use crate::error::{Error, ErrorCode};
use super::Either;
use serde::de::DeserializeOwned;
use std::time::Duration;

const DEFAULT_POLL_INTERVAL: usize = 5000; // 5 seconds

/// A trait for requestor types
pub trait TaskApi {
    /// Retrieve task result from the client. If the task is not completed yet, waits until it completes or cancels.
    fn get_task_result<T: DeserializeOwned>(&mut self, id: impl Into<String>) -> impl Future<Output = Result<T, Error>>;

    /// Retrieve task status from the client
    fn get_task(&mut self, id: impl Into<String>) -> impl Future<Output = Result<Task, Error>>;
    
    /// Cancels a task that is currently running on the client
    fn cancel_task(&mut self, id: impl Into<String>) -> impl Future<Output = Result<Task, Error>>;

    /// Retrieves a list of tasks from the client
    fn list_tasks(&mut self, cursor: Option<Cursor>) -> impl Future<Output = Result<ListTasksResult, Error>>;

    /// Input callback
    fn handle_input(&mut self, id: &str, params: TaskPayload) -> impl Future<Output = Result<(), Error>>;
}

/// Polls receiver with `tasks/get` until it completed, failed, cancelled or expired.
/// Call `tasks/result` if it completed or failed and `tasks/cancel` if expired.
pub async fn wait_to_completion<A, T>(
    api: &mut A, 
    result: Either<CreateTaskResult, T>
) -> Result<T, Error>
where 
    A: TaskApi,
    T: DeserializeOwned
{
    let mut task = match result {
        Either::Right(result) => return Ok(result),
        Either::Left(task_result) => task_result.task,
    };

    let mut elapsed = 0;
        
    loop {
        if task.ttl <= elapsed {
            #[cfg(feature = "tracing")]
            tracing::trace!(logger = "neva", "Task TTL expired. Cancelling task.");
            
            let _ = api.cancel_task(&task.id).await?;
            return Err(Error::new(ErrorCode::InvalidRequest, "Task was cancelled: TTL expired"));
        }
            
        task = api.get_task(&task.id).await?;
        
        match task.status {
            TaskStatus::Completed | TaskStatus::Failed => return api
                .get_task_result(&task.id)
                .await,
            TaskStatus::Cancelled => return Err(
                Error::new(ErrorCode::InvalidRequest, "Task was cancelled")
            ),
            TaskStatus::InputRequired => {
                #[cfg(feature = "tracing")]
                tracing::trace!(logger = "neva", "Task input required. Providing input.");
                
                let params: TaskPayload = api
                    .get_task_result(&task.id)
                    .await?;
                api.handle_input(&task.id, params).await?;
            },
            _ => {
                let poll_interval = task
                    .poll_interval
                    .unwrap_or(DEFAULT_POLL_INTERVAL);

                elapsed += poll_interval;

                #[cfg(feature = "tracing")]
                tracing::trace!(
                    logger = "neva", 
                    "Waiting for task to complete. Elapsed: {elapsed}ms");
                
                tokio::time::sleep(Duration::from_millis(poll_interval as u64)).await;
            }
        }
    }
}
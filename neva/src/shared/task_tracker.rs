//! Types and utilities for tracking tasks

use serde::Serialize;
use tokio_util::sync::{CancellationToken, WaitForCancellationFuture};
use tokio::sync::watch::{channel, Sender, Receiver};
use crate::error::{Error, ErrorCode};
use crate::types::{Task, TaskPayload, TaskStatus};

#[derive(Default)]
pub(crate) struct TaskTracker {
    tasks: dashmap::DashMap<String, TaskEntry>
}

/// Alias for [`Option<TaskPayload>`]
pub(crate) type MaybePayload = Option<TaskPayload>;

/// Represents a task currently running on the server
pub(crate) struct TaskEntry {
    task: Task,
    token: CancellationToken,
    #[cfg(feature = "server")]
    tx: Sender<MaybePayload>,
    rx: Receiver<MaybePayload>,
}

/// Represents a handle to a task that can be used to cancel or get the result of the task.
pub(crate) struct TaskHandle {
    token: CancellationToken,
    tx: Sender<MaybePayload>,
}

impl TaskTracker {
    /// Creates a new [`TaskTracker`]
    #[inline]
    pub(crate) fn new() -> Self {
        Self {
            tasks: dashmap::DashMap::new()
        }
    }
    
    /// Returns a list of currently running tasks.
    pub(crate) fn tasks(&self) -> Vec<Task> {
        self.tasks
            .iter()
            .map(|entry| entry.task.clone())
            .collect::<Vec<_>>()
    }

    /// Tacks the task and returns the [`TaskHandle`] for this task
    pub(crate) fn track(&self, task: Task) -> TaskHandle {
        let token = CancellationToken::new();
        let (tx, rx) = channel(None);
        
        self.tasks.insert(task.id.clone(), TaskEntry {
            token: token.clone(),
            #[cfg(feature = "server")]
            tx: tx.clone(),
            task,
            rx,
        });
        
        TaskHandle { token, tx }
    }

    /// Cancels the task
    pub(crate) fn cancel(&self, id: &str) -> Result<Task, Error> {
        if let Some((_, entry)) = self.tasks.remove(id) {
            entry.token.cancel();
            Ok(entry.task.cancel())
        } else {
            Err(Error::new(
                ErrorCode::InvalidParams,
                format!("Could not find task with id: {id}")))
        }
    }

    /// Completes the task
    pub(crate) fn complete(&self, id: &str) {
        if let Some(mut entry) = self.tasks.get_mut(id) {
            entry.task.complete();
        }
    }

    /// Fails the task
    #[cfg(feature = "server")]
    pub(crate) fn fail(&self, id: &str) {
        if let Some(mut entry) = self.tasks.get_mut(id) {
            entry.task.fail();
        }
    }

    /// Sets the task into `input_required` status
    #[cfg(feature = "server")]
    pub(crate) fn require_input(&self, id: &str) {
        if let Some(mut entry) = self.tasks.get_mut(id) {
            entry.task.require_input();
        }
    }

    /// Sets the task into `working` status
    #[cfg(feature = "server")]
    pub(crate) fn reset(&self, id: &str) {
        if let Some(mut entry) = self.tasks.get_mut(id) {
            entry.task.reset();
            let _ = entry
                .tx
                .send(None);
        }
    }

    /// Sets the result of the [`Task`].
    #[cfg(feature = "server")]
    pub(crate) fn set_result<T: Serialize>(&self, id: &str, result: T) {
        if let Some(entry) = self.tasks.get(id) {
            let result = match serde_json::to_value(result) {
                Ok(result) => result,
                Err(_err) => {
                    #[cfg(feature = "tracing")]
                    tracing::error!(
                        logger = "neva",
                        "Unable to serialize task result: {_err:?}");
                    return;
                }
            };
            let _ = entry
                .tx
                .send(Some(TaskPayload(result)));
        }
    }

    /// Retrieves the task status 
    pub(crate) fn get_status(&self, id: &str) -> Result<Task, Error> {
        self.tasks
            .get(id)
            .map(|t| t.task.clone())
            .ok_or_else(|| Error::new(
                ErrorCode::InvalidParams,
                format!("Could not find task with id: {id}")))
    }

    /// Returns the task result if it is present, 
    /// otherwise waits until the result is available or the task will be canceled.
    pub(crate) async fn get_result(&self, id: &str) -> Result<TaskPayload, Error> {
        let (status, mut result_rx, token) = {
            let entry = self.tasks
                .get(id)
                .ok_or_else(|| Error::new(
                    ErrorCode::InvalidParams,
                    format!("Could not find task with id: {id}")))?;

            (
                entry.task.status,
                entry.rx.clone(),
                entry.token.clone(),
            )
        };

        if let Some(ref result) = *result_rx.borrow_and_update() {
            if status != TaskStatus::InputRequired {
                self.tasks.remove(id);
            }
            return Ok(result.clone());
        }

        loop {
            tokio::select! {
                changed = result_rx.changed() => {
                    if changed.is_err() {
                        return Err(Error::new(ErrorCode::InternalError, "Unable to get task result"));
                    }

                    if let Some(result) = result_rx.borrow_and_update().clone() {
                        let task = self.get_status(id)?;
                        if task.status != TaskStatus::InputRequired {
                            self.tasks.remove(id);
                        }
                        return Ok(result);
                    }
                }
                _ = token.cancelled() => {
                    return Err(Error::new(ErrorCode::InvalidRequest, "Task has been cancelled"));
                }
            }
        }
    }
}

impl TaskHandle {
    /// Completes the [`Task`] and sets the result.
    pub(crate) fn set_result<T: Serialize>(self, result: T) {
        let result = match serde_json::to_value(result) {
            Ok(result) => result,
            Err(_err) => {
                #[cfg(feature = "tracing")]
                tracing::error!(
                    logger = "neva",
                    "Unable to serialize task result: {_err:?}");
                return;
            }
        };
        let _ = self.tx.send(Some(TaskPayload(result)));
    }

    /// Returns a [`Future`] that gets fulfilled when cancellation is requested.
    ///
    /// Equivalent to:
    ///
    /// ```ignore
    /// async fn cancelled(&self);
    /// ```
    ///
    /// The future will complete immediately if the token is already canceled
    /// when this method is called.
    ///
    /// # Cancellation safety
    ///
    /// This method is cancel safe.
    #[inline]
    pub(crate) fn cancelled(&self) -> WaitForCancellationFuture<'_> {
        self.token.cancelled()
    }
}


#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use super::*;
    use crate::types::TaskStatus;

    #[cfg(feature = "server")]
    use crate::types::CallToolResponse;

    #[test]
    fn it_can_create_new_tracker() {
        let tracker = TaskTracker::new();
        assert_eq!(tracker.tasks().len(), 0);
    }

    #[test]
    fn it_can_track_task() {
        let tracker = TaskTracker::new();
        let task = Task::new();
        let task_id = task.id.clone();

        let _handle = tracker.track(task);

        let tasks = tracker.tasks();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, task_id);
    }

    #[test]
    fn it_can_return_list_of_tasks() {
        let tracker = TaskTracker::new();
        let task1 = Task::new();
        let task2 = Task::new();

        let _handle1 = tracker.track(task1.clone());
        let _handle2 = tracker.track(task2.clone());

        let tasks = tracker.tasks();
        assert_eq!(tasks.len(), 2);
    }

    #[test]
    fn it_can_cancel_task() {
        let tracker = TaskTracker::new();
        let task = Task::new();
        let task_id = task.id.clone();

        let _handle = tracker.track(task);

        let result = tracker.cancel(&task_id).unwrap();
        assert_eq!(result.status, TaskStatus::Cancelled);
        assert_eq!(tracker.tasks().len(), 0);
    }

    #[test]
    fn it_does_return_error_when_cancelling_nonexistent_task() {
        let tracker = TaskTracker::new();

        let result = tracker.cancel("nonexistent");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::InvalidParams);
    }

    #[test]
    fn it_can_complete_task() {
        let tracker = TaskTracker::new();
        let task = Task::new();
        let task_id = task.id.clone();

        let _handle = tracker.track(task);

        tracker.complete(&task_id);

        let status = tracker.get_status(&task_id).unwrap();
        assert_eq!(status.status, TaskStatus::Completed);
    }

    #[test]
    fn it_does_nothing_when_completing_nonexistent_task() {
        let tracker = TaskTracker::new();
        tracker.complete("nonexistent");
        // Should not panic
    }

    #[cfg(feature = "server")]
    #[test]
    fn it_can_fail_task() {
        let tracker = TaskTracker::new();
        let task = Task::new();
        let task_id = task.id.clone();

        let _handle = tracker.track(task);

        tracker.fail(&task_id);

        let status = tracker.get_status(&task_id).unwrap();
        assert_eq!(status.status, TaskStatus::Failed);
    }

    #[cfg(feature = "server")]
    #[test]
    fn it_does_nothing_when_failing_nonexistent_task() {
        let tracker = TaskTracker::new();
        tracker.fail("nonexistent");
        // Should not panic
    }

    #[cfg(feature = "server")]
    #[test]
    fn it_can_require_input() {
        let tracker = TaskTracker::new();
        let task = Task::new();
        let task_id = task.id.clone();

        let _handle = tracker.track(task);

        tracker.require_input(&task_id);

        let status = tracker.get_status(&task_id).unwrap();
        assert_eq!(status.status, TaskStatus::InputRequired);
    }

    #[cfg(feature = "server")]
    #[test]
    fn it_does_nothing_when_requiring_input_for_nonexistent_task() {
        let tracker = TaskTracker::new();
        tracker.require_input("nonexistent");
        // Should not panic
    }

    #[test]
    fn it_can_get_task_status() {
        let tracker = TaskTracker::new();
        let task = Task::new();
        let task_id = task.id.clone();

        let _handle = tracker.track(task.clone());

        let status = tracker.get_status(&task_id).unwrap();
        assert_eq!(status.id, task.id);
        assert_eq!(status.status, TaskStatus::Working);
    }

    #[test]
    fn it_does_return_error_when_getting_status_of_nonexistent_task() {
        let tracker = TaskTracker::new();

        let result = tracker.get_status("nonexistent");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn it_can_get_task_result_when_completed() {
        let tracker = TaskTracker::new();
        let task = Task::new();
        let task_id = task.id.clone();

        let handle = tracker.track(task.clone());

        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            handle.set_result("test_result".to_string());
        });

        let result = tracker.get_result(&task_id).await.unwrap();
        assert_eq!(result.0, "test_result");
    }

    #[tokio::test]
    async fn it_does_return_result_immediately_when_already_available() {
        let tracker = TaskTracker::new();
        let task = Task::new();
        let task_id = task.id.clone();

        let handle = tracker.track(task.clone());
        handle.set_result("immediate_result".to_string());

        let result = tracker.get_result(&task_id).await.unwrap();
        assert_eq!(result.0, "immediate_result");
    }

    #[tokio::test]
    async fn it_does_return_error_when_getting_result_of_nonexistent_task() {
        let tracker = TaskTracker::new();

        let result = tracker.get_result("nonexistent").await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn it_does_return_error_when_task_is_cancelled() {
        let tracker = TaskTracker::new();
        let task = Task::new();
        let task_id = task.id.clone();

        let _handle = tracker.track(task.clone());
        
        let tracker = Arc::new(tracker);

        tokio::spawn({
            let tracker = tracker.clone();
            let task_id = task_id.clone();
            async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                let _ = tracker.cancel(&task_id);
            }
        });

        let result = tracker.get_result(&task_id).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::InvalidRequest);
    }

    #[tokio::test]
    async fn it_can_wait_for_result_with_multiple_updates() {
        let tracker = TaskTracker::new();
        let task = Task::new();
        let task_id = task.id.clone();

        let handle = tracker.track(task.clone());

        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            handle.set_result("final_result".to_string());
        });

        let result = tracker.get_result(&task_id).await.unwrap();
        assert_eq!(result.0, "final_result");
        assert_eq!(tracker.tasks().len(), 0);
    }

    #[tokio::test]
    async fn it_does_remove_task_after_getting_result() {
        let tracker = TaskTracker::new();
        let task = Task::new();
        let task_id = task.id.clone();

        let handle = tracker.track(task.clone());
        handle.set_result("result".to_string());

        let _ = tracker.get_result(&task_id).await.unwrap();

        assert_eq!(tracker.tasks().len(), 0);
    }

    #[tokio::test]
    async fn it_can_create_task_handle() {
        let tracker = TaskTracker::new();
        let task = Task::new();

        let handle = tracker.track(task);

        // Just ensure the handle can be created and used
        tokio::spawn(async move {
            tokio::select! {
                _ = handle.cancelled() => {}
            }
        });
    }

    #[tokio::test]
    async fn it_can_cancel_via_handle() {
        let tracker = TaskTracker::new();
        let task = Task::new();
        let task_id = task.id.clone();

        let handle = tracker.track(task.clone());

        let tracker = Arc::new(tracker);
        
        tokio::spawn({
            let tracker = tracker.clone();
            let task_id = task_id.clone();
            async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                let _ = tracker.cancel(&task_id);
            }
        });

        tokio::select! {
            _ = handle.cancelled() => {
                // Successfully cancelled
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(1)) => {
                panic!("Task was not cancelled");
            }
        }
    }

    #[test]
    #[cfg(feature = "server")]
    fn it_can_handle_complex_payload_types() {
        let tracker = TaskTracker::new();
        let task = Task::new();
        let task_id = task.id.clone();

        let handle = tracker.track(task.clone());

        let response = CallToolResponse::new("test");
        tracker.complete(&task_id);
        handle.set_result(response.clone());

        let status = tracker.get_status(&task_id).unwrap();
        assert_eq!(status.status, TaskStatus::Completed);
    }

    #[tokio::test]
    async fn it_can_track_multiple_concurrent_tasks() {
        let tracker = TaskTracker::new();
        let tasks: Vec<_> = (0..5).map(|_| Task::new()).collect();
        let task_ids: Vec<_> = tasks.iter().map(|t| t.id.clone()).collect();

        let handles: Vec<_> = tasks.into_iter().map(|t| tracker.track(t)).collect();

        for (i, handle) in handles.into_iter().enumerate() {
            let result = format!("result_{}", i);
            handle.set_result(result);
        }

        for (i, task_id) in task_ids.iter().enumerate() {
            let result = tracker.get_result(task_id).await.unwrap();
            assert_eq!(result.0, format!("result_{}", i));
        }

        assert_eq!(tracker.tasks().len(), 0);
    }

    #[test]
    fn it_does_maintain_task_state_transitions() {
        let tracker = TaskTracker::new();
        let task = Task::new();
        let task_id = task.id.clone();

        let _handle = tracker.track(task.clone());

        let status = tracker.get_status(&task_id).unwrap();
        assert_eq!(status.status, TaskStatus::Working);

        tracker.complete(&task_id);
        let status = tracker.get_status(&task_id).unwrap();
        assert_eq!(status.status, TaskStatus::Completed);
    }
}
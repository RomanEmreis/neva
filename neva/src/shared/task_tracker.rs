//! Types and utilities for tracking tasks

use tokio_util::sync::CancellationToken;
use tokio::sync::watch::{channel, Sender};
use crate::error::{Error, ErrorCode};
use crate::types::{Task, TaskPayload};

#[derive(Default)]
pub(crate) struct TaskTracker<T> {
    tasks: dashmap::DashMap<String, TaskEntry<T>>
}

/// Alias for [`Option<TaskPayload<T>>`]
pub(crate) type MaybePayload<T> = Option<TaskPayload<T>>;

/// Represents a task currently running on the server
pub(crate) struct TaskEntry<T> {
    task: Task,
    token: CancellationToken,
    result_tx: Sender<MaybePayload<T>>,
}

/// Represents a handle to a task that can be used to cancel or complete it
pub(crate) struct TaskHandle<T> {
    pub(crate) token: CancellationToken,
    result_tx: Sender<MaybePayload<T>>
}

impl<T: Clone> TaskTracker<T> {
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
    pub(crate) fn track(&self, task: Task) -> TaskHandle<T> {
        let id = task.id.clone();
        let entry = TaskEntry::new(task);
        let handle = entry.get_handle();
        self.tasks.insert(id, entry);
        handle
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
    pub(crate) fn complete(&self, id: &str, result: T) {
        if let Some(mut entry) = self.tasks.get_mut(id) {
            entry.task.complete();
            let _ = entry.result_tx.send(Some(TaskPayload(result)));
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
    pub(crate) async fn get_result(&self, id: &str) -> Result<TaskPayload<T>, Error> {
        let (mut result_rx, token) = {
            let entry = self.tasks
                .get(id)
                .ok_or_else(|| Error::new(
                    ErrorCode::InvalidParams,
                    format!("Could not find task with id: {id}")))?;

            if let Some(ref result) = *entry.result_tx.borrow() {
                return Ok(result.clone());
            }

            (
                entry.result_tx.subscribe(),
                entry.token.clone(),
            )
        };

        loop {
            tokio::select! {
                changed = result_rx.changed() => {
                    if changed.is_err() {
                        return Err(Error::new(ErrorCode::InternalError, "Unable to get task result"));
                    }

                    if let Some(result) = result_rx.borrow_and_update().clone() {
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

impl<T> TaskEntry<T> {
    /// Creates a new [`TaskEntry`] for the given `task`
    #[inline]
    pub(crate) fn new(task: Task) -> Self {
        let token = CancellationToken::new();
        let (tx, _rx) = channel(None);
        Self {
            result_tx: tx,
            token,
            task,
        }
    }

    /// Creates a new [`TaskHandle`] for the given [`TaskEntry`]
    #[inline]
    pub(crate) fn get_handle(&self) -> TaskHandle<T> {
        TaskHandle {
            result_tx: self.result_tx.clone(),
            token: self.token.clone()
        }
    }
}

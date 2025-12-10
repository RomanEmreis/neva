//! Types and utilities for tracking tasks

use tokio_util::sync::{CancellationToken, WaitForCancellationFuture};
use tokio::sync::watch::{channel, Sender, Receiver};
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
    rx: Receiver<MaybePayload<T>>,
}

/// Represents a handle to a task that can be used to cancel or get the result of the task.
pub(crate) struct TaskHandle<T> {
    token: CancellationToken,
    tx: Sender<MaybePayload<T>>,
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
        let token = CancellationToken::new();
        let (tx, rx) = channel(None);
        
        self.tasks.insert(task.id.clone(), TaskEntry {
            token: token.clone(),
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

            (
                entry.rx.clone(),
                entry.token.clone(),
            )
        };

        if let Some(ref result) = *result_rx.borrow_and_update() {
            self.tasks.remove(id);
            return Ok(result.clone());
        }

        loop {
            tokio::select! {
                changed = result_rx.changed() => {
                    if changed.is_err() {
                        return Err(Error::new(ErrorCode::InternalError, "Unable to get task result"));
                    }

                    if let Some(result) = result_rx.borrow_and_update().clone() {
                        self.tasks.remove(id);
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

impl<T> TaskHandle<T> {
    /// Completes the [`Task`] and sets the result.
    pub(crate) fn complete(self, result: T) {
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
    /// The future will complete immediately if the token is already cancelled
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

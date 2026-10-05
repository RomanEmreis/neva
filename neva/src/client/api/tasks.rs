//! `tasks/*`: following the tasks a task-augmented call started.
//!
//! The methods are the ones the build's protocol generation has: MCP
//! 2026-07-28 drops `tasks/list` and `tasks/result` and adds `tasks/update`.

use super::Client;
use crate::error::Error;
use crate::shared::TaskApi;
use std::fmt::{Debug, Formatter};

#[cfg(not(feature = "legacy-spec"))]
use crate::types::{DetailedTask, mrtr::InputResponses};
#[cfg(feature = "legacy-spec")]
use crate::types::{ListTasksResult, Task, cursor::Cursor};
#[cfg(feature = "legacy-spec")]
use serde::de::DeserializeOwned;

/// The `tasks/*` methods of a connected server, from [`Client::tasks`].
///
/// # Examples
/// ```no_run
/// use neva::client::Client;
/// use neva::error::Error;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Error> {
///     let mut client = Client::new();
///     client.connect().await?;
///
///     let tasks = client.tasks();
///     let task = tasks.get("task-1").await?;
///     tasks.cancel("task-1").await?;
///
///     client.disconnect().await
/// }
/// ```
#[derive(Clone, Copy)]
pub struct Tasks<'a> {
    client: &'a Client,
}

impl Debug for Tasks<'_> {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tasks").finish_non_exhaustive()
    }
}

impl<'a> Tasks<'a> {
    #[inline]
    pub(super) fn new(client: &'a Client) -> Self {
        Self { client }
    }

    /// Retrieves the full state of the task `id` (`tasks/get`): its status
    /// plus, depending on it, the outstanding input requests, the terminal
    /// result, or the error.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///     client.connect().await?;
    ///
    ///     let task = client.tasks().get("task-1").await?;
    ///     println!("{:?}", task.status);
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub async fn get(&self, id: impl Into<String>) -> Result<DetailedTask, Error> {
        self.client.get_task(id).await
    }

    /// Retrieves the status of the task `id` (`tasks/get`).
    ///
    /// # Examples
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///     client.connect().await?;
    ///
    ///     let task = client.tasks().get("task-1").await?;
    ///     println!("{:?}", task.status);
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    #[cfg(feature = "legacy-spec")]
    pub async fn get(&self, id: impl Into<String>) -> Result<Task, Error> {
        self.client.get_task(id).await
    }

    /// Answers the outstanding input requests of the task `id`
    /// (`tasks/update`).
    ///
    /// # Examples
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    /// use neva::types::mrtr::InputResponses;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///     client.connect().await?;
    ///
    ///     client.tasks().update("task-1", InputResponses::new()).await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub async fn update(
        &self,
        id: impl Into<String>,
        responses: InputResponses,
    ) -> Result<(), Error> {
        self.client.update_task(id, responses).await
    }

    /// Asks the server to cancel the task `id` (`tasks/cancel`).
    ///
    /// Cancellation is cooperative: the acknowledgement means the request was
    /// received, not that the task stopped. [`Self::get`] tells the outcome.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///     client.connect().await?;
    ///
    ///     client.tasks().cancel("task-1").await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub async fn cancel(&self, id: impl Into<String>) -> Result<(), Error> {
        self.client.cancel_task(id).await
    }

    /// Asks the server to cancel the task `id` (`tasks/cancel`), returning its
    /// state after the request.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///     client.connect().await?;
    ///
    ///     let task = client.tasks().cancel("task-1").await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    #[cfg(feature = "legacy-spec")]
    pub async fn cancel(&self, id: impl Into<String>) -> Result<Task, Error> {
        self.client.cancel_task(id).await
    }

    /// Retrieves the result of the task `id` (`tasks/result`), waiting for it
    /// to finish if it has not.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    /// use neva::types::CallToolResponse;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///     client.connect().await?;
    ///
    ///     let result: CallToolResponse = client.tasks().result("task-1").await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    #[cfg(feature = "legacy-spec")]
    pub async fn result<T: DeserializeOwned>(&self, id: impl Into<String>) -> Result<T, Error> {
        self.client.get_task_result(id).await
    }

    /// Requests one page of the server's tasks (`tasks/list`).
    ///
    /// # Examples
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///     client.connect().await?;
    ///
    ///     let tasks = client.tasks().list(None).await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    #[cfg(feature = "legacy-spec")]
    pub async fn list(&self, cursor: Option<Cursor>) -> Result<ListTasksResult, Error> {
        self.client.list_tasks(cursor).await
    }
}

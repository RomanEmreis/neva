//! Types and utilities for task-augmented requests and responses

use std::ops::{Deref, DerefMut};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use chrono::{DateTime, Utc};
use serde_json::Value;
use crate::{
    types::{Meta, Cursor, IntoResponse, Page, RequestId, Response},
    error::Error
};

#[cfg(feature = "server")]
use crate::{
    app::handler::{FromHandlerParams, HandlerParams},
    types::request::{FromRequest, Request}
};

pub(crate) const RELATED_TASK_KEY: &str = "io.modelcontextprotocol/related-task";

const DEFAULT_TTL: usize = 30000;

/// List of commands for Tasks
pub mod commands {
    /// Command name that returns a list of tasks that are currently running on the server.
    pub const LIST: &str = "tasks/list";
    
    /// Command name that cancels a task on the server.
    pub const CANCEL: &str = "tasks/cancel";
    
    /// Command name that returns the result of a task.
    pub const RESULT: &str = "tasks/result";
    
    /// Command name that returns the status of a task.
    pub const GET: &str = "tasks/get";
    
    /// Notification name that notifies the client about the status of a task.
    pub const STATUS: &str = "notifications/tasks/status";
}

/// Represents a request to retrieve a list of tasks.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct ListTasksRequestParams {
    /// An opaque token representing the current pagination position.
    /// If provided, the server should return results starting after this cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
}

/// Represents the response to a `tasks/list` request.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct ListTasksResult {
    /// A list of tasks that the server currently runs.
    pub tasks: Vec<Task>,
    
    /// An opaque token representing the pagination position after the last returned result.
    ///
    /// When a paginated result has more data available, the `next_cursor`
    /// field will contain `Some` token that can be used in subsequent requests
    /// to fetch the next page. When there are no more results to return, the `next_cursor` field
    /// will be `None`.
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// Represents a request to cancel a task.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct CancelTaskRequestParams {
    /// The task identifier to cancel.
    #[serde(rename = "taskId")]
    pub id: String
}

/// Represents a request to retrieve the state of a task.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct GetTaskRequestParams {
    /// The task identifier to retrieve the state for.
    #[serde(rename = "taskId")]
    pub id: String
}

/// Represents a request to retrieve the result of a completed task.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct GetTaskPayloadRequestParams {
    /// The task identifier to retrieve the result for.
    #[serde(rename = "taskId")]
    pub id: String
}

/// Represents a response to a task-augmented request.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct CreateTaskResult {
    /// Newly created task information
    pub task: Task,

    /// Metadata reserved by MCP for protocol-level metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// Represents a task. Tasks are durable state machines that carry information 
/// about the underlying execution state of the request they wrap, and are intended for requestor 
/// polling and deferred result retrieval. 
/// 
/// Each task is uniquely identifiable by a receiver-generated **task ID**.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// The task identifier.
    #[serde(rename = "taskId")]
    pub id: String,
    
    /// ISO 8601 timestamp when the task was created.
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    
    /// ISO 8601 timestamp when the task was last updated.
    #[serde(rename = "lastUpdatedAt")]
    pub last_updated_at: DateTime<Utc>,
    
    /// Time To Live: Actual retention duration from creation in milliseconds, null for unlimited.
    pub ttl: usize,
    
    /// Current task state.
    pub status: TaskStatus,
    
    /// Optional human-readable message describing the current task state.
    /// This can provide context for any status, including
    /// - Reasons for `cancelled` status
    /// - Summaries for `completed` status
    /// - Diagnostic information for `failed` status (e.g., error details, what went wrong)
    #[serde(rename = "statusMessage", skip_serializing_if = "Option::is_none")]
    pub status_msg: Option<String>,
    
    /// Suggested polling interval in milliseconds.
    #[serde(rename = "pollInterval", skip_serializing_if = "Option::is_none")]
    pub poll_interval: Option<usize>,
}

/// Represents the status of a task.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum TaskStatus {
    /// Task has been canceled.
    #[serde(rename = "cancelled")]
    Cancelled,
    
    /// Task has completed successfully.
    #[serde(rename = "completed")]
    Completed,
    
    /// Task has failed.
    #[serde(rename = "failed")]
    Failed,
    
    /// Task is currently running.
    #[default]
    #[serde(rename = "working")]
    Working,
    
    /// Task requires an input to proceed.
    #[serde(rename = "input_required")]
    InputRequired
}

/// Represents metadata for augmenting a request with a task execution.
/// Include this in the `task` field of the request parameters.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TaskMetadata {
    /// Time To Live: requested duration in milliseconds to retain task from creation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<usize>,
}

/// Represents metadata for associating messages with a task.
/// Include this in the `_meta` field under the key `io.modelcontextprotocol/related-task`.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct RelatedTaskMetadata {
    /// The task identifier this message is associated with.
    #[serde(rename = "taskId")]
    pub id: String,
}

/// Represents the response to a `tasks/result` request.
/// The inner `Value` matches the result type of the original request.
/// For example, a `tools/call` task would return the [`CallToolResponse`] structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPayload(pub Value);

impl Deref for TaskPayload {
    type Target = Value;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for TaskPayload {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl IntoResponse for Task {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        match serde_json::to_value(self) {
            Ok(v) => Response::success(req_id, v),
            Err(err) => Response::error(req_id, err.into())
        }
    }
}

impl IntoResponse for TaskPayload {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        self.0.into_response(req_id)
    }
}

impl IntoResponse for CreateTaskResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        match serde_json::to_value(self) {
            Ok(v) => Response::success(req_id, v),
            Err(err) => Response::error(req_id, err.into())
        }
    }
}

impl IntoResponse for ListTasksResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        match serde_json::to_value(self) {
            Ok(v) => Response::success(req_id, v),
            Err(err) => Response::error(req_id, err.into())
        }
    }
}

impl<const N: usize> From<[Task; N]> for ListTasksResult {
    #[inline]
    fn from(tasks: [Task; N]) -> Self {
        Self {
            next_cursor: None,
            tasks: tasks.to_vec()
        }
    }
}

impl From<Vec<Task>> for ListTasksResult {
    #[inline]
    fn from(tasks: Vec<Task>) -> Self {
        Self {
            next_cursor: None,
            tasks
        }
    }
}

impl From<Page<'_, Task>> for ListTasksResult {
    #[inline]
    fn from(page: Page<'_, Task>) -> Self {
        Self {
            next_cursor: page.next_cursor,
            tasks: page.items.to_vec()
        }
    }
}

impl<T: Into<String>> From<T> for RelatedTaskMetadata {
    #[inline]
    fn from(value: T) -> Self {
        Self { id: value.into() }
    }
}

impl From<Meta<RelatedTaskMetadata>> for RelatedTaskMetadata {
    #[inline]
    fn from(meta: Meta<RelatedTaskMetadata>) -> Self {
        meta.into_inner()
    }
}

#[cfg(feature = "server")]
impl FromHandlerParams for ListTasksRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

#[cfg(feature = "server")]
impl FromHandlerParams for CancelTaskRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

#[cfg(feature = "server")]
impl FromHandlerParams for GetTaskRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

#[cfg(feature = "server")]
impl FromHandlerParams for GetTaskPayloadRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

impl Default for Task {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl From<TaskMetadata> for Task {
    #[inline]
    fn from(meta: TaskMetadata) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            created_at: Utc::now(),
            last_updated_at: Utc::now(),
            ttl: meta.ttl.unwrap_or(DEFAULT_TTL),
            status: TaskStatus::Working,
            status_msg: None,
            poll_interval: None
        }
    }
}

impl ListTasksResult {
    /// Creates a new [`ListTasksResult`]
    #[inline]
    pub fn new() -> Self {
        Default::default()
    }
}

impl CreateTaskResult {
    /// Creates a new [`CreateTaskResult`]
    pub fn new(task: Task) -> Self {
        Self { task, meta: None }
    }
}

impl Task {
    /// Creates a new [`Task`] in `working` status and with a default TTL of 30 seconds.
    #[inline]
    pub fn new() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            created_at: Utc::now(),
            last_updated_at: Utc::now(),
            ttl: DEFAULT_TTL,
            status: TaskStatus::Working,
            status_msg: None,
            poll_interval: None
        }
    }
    
    /// Sets the status message of the task.
    pub fn set_message(&mut self, msg: impl Into<String>) {
        self.status_msg = Some(msg.into());
        self.last_updated_at = Utc::now();
    }

    /// Sets the `working` status.
    pub fn reset(&mut self) {
        self.status = TaskStatus::Working;
        self.last_updated_at = Utc::now();
    }

    /// Sets the `cancelled` status.
    pub fn cancel(mut self) -> Self {
        self.status = TaskStatus::Cancelled;
        self.last_updated_at = Utc::now();
        self
    }

    /// Sets the `completed` status.
    pub fn complete(&mut self) {
        self.status = TaskStatus::Completed;
        self.last_updated_at = Utc::now();
    }

    /// Sets the `failed` status.
    pub fn fail(&mut self) {
        self.status = TaskStatus::Failed;
        self.last_updated_at = Utc::now();
    }

    /// Sets the `input_required` status.
    pub fn require_input(&mut self) {
        self.status = TaskStatus::InputRequired;
        self.last_updated_at = Utc::now();
    }
}

impl TaskPayload {
    /// Unwraps the inner `Value`.
    #[inline]
    pub fn into_inner(self) -> Value {
        self.0
    }

    /// Unwraps the inner `T`
    #[inline]
    pub fn to<T: DeserializeOwned>(self) -> Result<T, Error> {
        serde_json::from_value::<T>(self.0)
            .map_err(Error::from)
    }
}

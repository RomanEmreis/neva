//! Types and utilities for task-augmented requests and responses

use crate::{
    error::Error,
    types::{IntoResponse, Meta, RequestId, Response},
};

#[cfg(feature = "legacy-spec")]
use crate::types::{Cursor, Page};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::ops::{Deref, DerefMut};

#[cfg(feature = "server")]
use crate::{
    app::handler::{FromHandlerParams, HandlerParams},
    types::request::{FromRequest, Request},
};

pub(crate) const RELATED_TASK_KEY: &str = "io.modelcontextprotocol/related-task";

/// Reverse-DNS id of the Tasks extension (MCP 2026-07-28). Under MCP 2026-07-28
/// the tasks capability is advertised under `capabilities.extensions[this id]`.
#[cfg(not(feature = "legacy-spec"))]
pub(crate) const TASKS_EXTENSION_ID: &str = "io.modelcontextprotocol/tasks";

const DEFAULT_TTL: usize = 30000;

/// List of commands for Tasks
pub mod commands {
    /// Command name that returns a list of tasks that are currently running on the server.
    ///
    /// Removed in MCP 2026-07-28: the final Tasks extension has no `tasks/list`.
    #[cfg(feature = "legacy-spec")]
    pub const LIST: &str = "tasks/list";

    /// Command name that cancels a task on the server.
    pub const CANCEL: &str = "tasks/cancel";

    /// Command name that returns the result of a task.
    ///
    /// Removed in MCP 2026-07-28: result retrieval folded into
    /// [`GET`], whose response carries the terminal `result` / `error` inline.
    #[cfg(feature = "legacy-spec")]
    pub const RESULT: &str = "tasks/result";

    /// Command name that returns the state of a task.
    ///
    /// Under MCP 2026-07-28 this is the single polling method: the response is a
    /// `DetailedTask` carrying the status *and*, for the relevant states, the
    /// outstanding `inputRequests`, the terminal `result`, or the `error`.
    /// Under `legacy-spec` it returns status only.
    pub const GET: &str = "tasks/get";

    /// Command name that submits client responses to a task's outstanding
    /// input requests (MCP 2026-07-28).
    #[cfg(not(feature = "legacy-spec"))]
    pub const UPDATE: &str = "tasks/update";

    /// Notification name that notifies the client about the status of a task.
    #[cfg(not(feature = "legacy-spec"))]
    pub const STATUS: &str = "notifications/tasks";

    /// Notification name that notifies the client about the status of a task.
    #[cfg(feature = "legacy-spec")]
    pub const STATUS: &str = "notifications/tasks/status";
}

/// Represents a request to retrieve a list of tasks.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[cfg(feature = "legacy-spec")]
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
#[cfg(feature = "legacy-spec")]
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
    pub id: String,
}

/// Represents a request to retrieve the state of a task.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct GetTaskRequestParams {
    /// The task identifier to retrieve the state for.
    #[serde(rename = "taskId")]
    pub id: String,
}

/// Represents a request to retrieve the result of a completed task.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[cfg(feature = "legacy-spec")]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct GetTaskPayloadRequestParams {
    /// The task identifier to retrieve the result for.
    #[serde(rename = "taskId")]
    pub id: String,
}

/// Represents a request to submit responses to a task's outstanding input
/// requests (`tasks/update`, MCP 2026-07-28).
///
/// A task that needs input moves to [`TaskStatus::InputRequired`] and surfaces
/// the pending asks in [`DetailedTask::input_requests`]. The client answers
/// them here rather than over a server-initiated channel -- there is none.
#[cfg(not(feature = "legacy-spec"))]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct UpdateTaskRequestParams {
    /// The task identifier to update.
    #[serde(rename = "taskId")]
    pub id: String,

    /// Responses to outstanding input requests. Each key must match a
    /// currently-outstanding [`DetailedTask::input_requests`] key; responses
    /// for unknown or already-satisfied keys are ignored.
    #[serde(rename = "inputResponses")]
    pub input_responses: crate::types::mrtr::InputResponses,
}

/// Discriminator for [`CreateTaskResult`], always `"task"`.
///
/// The third `resultType` value alongside `"complete"` and
/// `"input_required"`: it marks a result the server has deferred onto a task
/// instead of answering inline.
#[cfg(not(feature = "legacy-spec"))]
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub enum CreateTaskTag {
    /// The only variant.
    #[default]
    #[serde(rename = "task")]
    Task,
}

/// Represents a response to a task-augmented request.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct CreateTaskResult {
    /// Discriminator, always `"task"` (MCP 2026-07-28).
    #[cfg(not(feature = "legacy-spec"))]
    #[serde(rename = "resultType")]
    pub result_type: CreateTaskTag,

    /// Newly created task information.
    ///
    /// Under MCP 2026-07-28 the spec defines this result as `Result & Task`,
    /// so the task's own fields sit at the top level; `legacy-spec` keeps the
    /// nested `task` object of the earlier revision.
    #[cfg_attr(not(feature = "legacy-spec"), serde(flatten))]
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

    /// Time To Live: actual retention duration from creation in milliseconds,
    /// `None` for unlimited. The server may discard the task once it elapses,
    /// and the value may change over the task's lifetime.
    ///
    /// Serialized as `ttlMs` under MCP 2026-07-28 and as `ttl` under
    /// `legacy-spec`.
    #[cfg_attr(not(feature = "legacy-spec"), serde(rename = "ttlMs"))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<usize>,

    /// Current task state.
    pub status: TaskStatus,

    /// Optional human-readable message describing the current task state.
    /// This can provide context for any status, including
    /// - Reasons for `cancelled` status
    /// - Summaries for `completed` status
    /// - Diagnostic information for `failed` status (e.g., error details, what went wrong)
    #[serde(rename = "statusMessage", skip_serializing_if = "Option::is_none")]
    pub status_msg: Option<String>,

    /// Suggested polling interval in milliseconds. Clients should honor it to
    /// avoid overwhelming the server; it may change over the task's lifetime.
    ///
    /// Serialized as `pollIntervalMs` under MCP 2026-07-28 and as
    /// `pollInterval` under `legacy-spec`.
    #[cfg_attr(not(feature = "legacy-spec"), serde(rename = "pollIntervalMs"))]
    #[cfg_attr(feature = "legacy-spec", serde(rename = "pollInterval"))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poll_interval: Option<usize>,
}

/// A task with its status-specific fields inlined -- the shape `tasks/get`
/// returns and `notifications/tasks` carries (MCP 2026-07-28).
///
/// The spec models this as a union discriminated by [`Task::status`]:
/// `input_required` carries [`Self::input_requests`], `completed` carries
/// [`Self::result`], `failed` carries [`Self::error`], and `working` /
/// `cancelled` carry neither. neva models it as one struct with optional
/// fields so a peer that sends more than the status demands still parses.
///
/// # Examples
///
/// ```
/// use neva::types::{DetailedTask, Task};
///
/// let task = DetailedTask::from(Task::new());
/// assert!(task.result.is_none());
/// ```
#[cfg(not(feature = "legacy-spec"))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetailedTask {
    /// The task's own fields, flattened to the top level per the schema.
    #[serde(flatten)]
    pub task: Task,

    /// Outstanding server-to-client requests, present while the task is in
    /// [`TaskStatus::InputRequired`]. Keys are answered through
    /// [`UpdateTaskRequestParams::input_responses`].
    #[serde(rename = "inputRequests", skip_serializing_if = "Option::is_none")]
    pub input_requests: Option<crate::types::mrtr::InputRequests>,

    /// The final result, present once the task reaches
    /// [`TaskStatus::Completed`]. Matches the result type the original request
    /// would have returned synchronously.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,

    /// The JSON-RPC error that ended the task, present on
    /// [`TaskStatus::Failed`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
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
    InputRequired,
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
/// For example, a `tools/call` task would return the [`crate::types::CallToolResponse`] structure.
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
            Err(err) => Response::error(req_id, err.into()),
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
            Err(err) => Response::error(req_id, err.into()),
        }
    }
}

#[cfg(not(feature = "legacy-spec"))]
impl IntoResponse for DetailedTask {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        match serde_json::to_value(self) {
            Ok(v) => Response::success(req_id, v),
            Err(err) => Response::error(req_id, err.into()),
        }
    }
}

#[cfg(not(feature = "legacy-spec"))]
impl From<Task> for DetailedTask {
    #[inline]
    fn from(task: Task) -> Self {
        Self {
            task,
            input_requests: None,
            result: None,
            error: None,
        }
    }
}

#[cfg(not(feature = "legacy-spec"))]
impl Deref for DetailedTask {
    type Target = Task;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.task
    }
}

#[cfg(not(feature = "legacy-spec"))]
impl DerefMut for DetailedTask {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.task
    }
}

#[cfg(feature = "legacy-spec")]
impl IntoResponse for ListTasksResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        match serde_json::to_value(self) {
            Ok(v) => Response::success(req_id, v),
            Err(err) => Response::error(req_id, err.into()),
        }
    }
}

#[cfg(feature = "legacy-spec")]
impl<const N: usize> From<[Task; N]> for ListTasksResult {
    #[inline]
    fn from(tasks: [Task; N]) -> Self {
        Self {
            next_cursor: None,
            tasks: tasks.to_vec(),
        }
    }
}

#[cfg(feature = "legacy-spec")]
impl From<Vec<Task>> for ListTasksResult {
    #[inline]
    fn from(tasks: Vec<Task>) -> Self {
        Self {
            next_cursor: None,
            tasks,
        }
    }
}

#[cfg(feature = "legacy-spec")]
impl From<Page<'_, Task>> for ListTasksResult {
    #[inline]
    fn from(page: Page<'_, Task>) -> Self {
        Self {
            next_cursor: page.next_cursor,
            tasks: page.items.to_vec(),
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

#[cfg(all(feature = "server", feature = "legacy-spec"))]
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

#[cfg(all(feature = "server", feature = "legacy-spec"))]
impl FromHandlerParams for GetTaskPayloadRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

#[cfg(all(feature = "server", not(feature = "legacy-spec")))]
impl FromHandlerParams for UpdateTaskRequestParams {
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
            ttl: Some(meta.ttl.unwrap_or(DEFAULT_TTL)),
            status: TaskStatus::Working,
            status_msg: None,
            poll_interval: None,
        }
    }
}

#[cfg(feature = "legacy-spec")]
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
        Self {
            #[cfg(not(feature = "legacy-spec"))]
            result_type: CreateTaskTag::Task,
            task,
            meta: None,
        }
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
            ttl: Some(DEFAULT_TTL),
            status: TaskStatus::Working,
            status_msg: None,
            poll_interval: None,
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
        serde_json::from_value::<T>(self.0).map_err(Error::from)
    }
}

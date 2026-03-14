//! Types used by the MCP protocol
//! 
//! See the [specification](https://github.com/modelcontextprotocol/specification) for details

use std::fmt::Display;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::SDK_NAME;
use crate::types::notification::Notification;

#[cfg(feature = "server")]
use crate::{
    app::handler::{FromHandlerParams, HandlerParams},
    app::options::McpOptions,
    error::Error,
};

#[cfg(feature = "server")]
pub use request::FromRequest;

#[cfg(feature = "http-server")]
use {
    crate::auth::DefaultClaims,
    volga::headers::HeaderMap
};

pub use helpers::{Json, Meta, PropertyType};
pub use request::{RequestId, Request, RequestParamsMeta};
pub use response::{IntoResponse, Response, ErrorDetails};
pub use reference::Reference;
pub use completion::{Completion, CompleteRequestParams, Argument, CompleteResult};
pub use cursor::{Cursor, Page, Pagination};
pub use content::{
    Content, 
    TextContent, 
    AudioContent, 
    ImageContent,
    ResourceLink,
    EmbeddedResource,
    ToolUse,
    ToolResult
};
pub use capabilities::{
    ClientCapabilities, 
    ServerCapabilities, 
    ToolsCapability, 
    ResourcesCapability,
    PromptsCapability,
    LoggingCapability,
    CompletionsCapability,
    ElicitationCapability,
    ElicitationFormCapability,
    ElicitationUrlCapability,
    SamplingCapability,
    SamplingContextCapability,
    SamplingToolsCapability,
    RootsCapability
};

#[cfg(feature = "tasks")]
pub use capabilities::{
    ServerTasksCapability, 
    ClientTasksCapability,
    TaskListCapability,
    TaskCancellationCapability,
    ClientTaskRequestsCapability,
    ServerTaskRequestsCapability,
    ToolsTaskCapability,
    ToolsCallTaskCapability,
    SamplingTaskCapability,
    SamplingCreateMessageTaskCapability,
    ElicitationTaskCapability,
    ElicitationCreateTaskCapability
};

pub use tool::{
    ListToolsRequestParams,
    CallToolRequestParams,
    CallToolResponse,
    Tool,
    ToolSchema,
    ToolAnnotations,
    ListToolsResult
};

#[cfg(feature = "server")]
pub use tool::ToolHandler;

pub use resource::{
    Uri,
    ListResourcesRequestParams,
    ListResourceTemplatesRequestParams,
    ListResourcesResult,
    ListResourceTemplatesResult,
    Resource,
    ResourceTemplate,
    ResourceContents,
    TextResourceContents,
    BlobResourceContents,
    ReadResourceResult, 
    ReadResourceRequestParams,
    SubscribeRequestParams,
    UnsubscribeRequestParams,
};
pub use prompt::{
    ListPromptsRequestParams,
    ListPromptsResult,
    Prompt,
    GetPromptRequestParams,
    GetPromptResult,
    PromptArgument,
    PromptMessage,
};
pub use sampling::{
    CreateMessageRequestParams,
    CreateMessageResult,
    SamplingMessage,
    StopReason,
    ToolChoiceMode,
    ToolChoice
};
pub use elicitation::{
    UrlElicitationRequiredError,
    ElicitationCompleteParams,
    ElicitRequestParams,
    ElicitRequestFormParams,
    ElicitRequestUrlParams,
    ElicitationAction,
    ElicitationMode,
    ElicitResult
};
pub use schema::{
    Schema,
    StringSchema,
    StringFormat,
    NumberSchema,
    BooleanSchema,
    TitledMultiSelectEnumSchema,
    TitledSingleSelectEnumSchema,
    UntitledMultiSelectEnumSchema,
    UntitledSingleSelectEnumSchema,
};

pub use icon::{
    Icon, 
    IconSize, 
    IconTheme,
};

#[cfg(feature = "tasks")]
pub use task::{
    GetTaskPayloadRequestParams,
    GetTaskRequestParams,
    ListTasksRequestParams,
    ListTasksResult,
    CancelTaskRequestParams,
    CreateTaskResult,
    RelatedTaskMetadata,
    TaskMetadata,
    TaskPayload,
    TaskStatus,
    Task,
};

#[cfg(feature = "server")]
pub use prompt::PromptHandler;

pub use root::Root;
pub use progress::ProgressToken;

mod request;
mod response;
mod capabilities;
mod reference;
mod content;
mod progress;
mod schema;
pub mod tool;
pub mod resource;
pub mod prompt;
pub mod completion;
pub mod notification;
pub mod cursor;
pub mod root;
pub mod sampling;
pub mod elicitation;
#[cfg(feature = "tasks")]
pub mod task;
mod icon;
pub(crate) mod helpers;

pub(super) const JSONRPC_VERSION: &str = "2.0";

/// Represents a JSON RPC message that could be either [`Request`] or [`Response`] or [`Notification`] or a [`MessageBatch`]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    /// See [`Request`]
    Request(Request),

    /// See [`Response`]
    Response(Response),

    /// See [`Notification`]
    Notification(Notification),

    /// See [`MessageBatch`]
    ///
    /// # Note
    /// This variant **must remain last**. `#[serde(untagged)]` tries variants in
    /// declaration order. A JSON array will always fail to deserialize as the
    /// object-shaped `Request`, `Response`, and `Notification` variants above,
    /// so placing `Batch` last is both safe and correct.
    Batch(MessageBatch),
}

/// Represents a single JSON-RPC message inside a batch.
/// Batches cannot be nested, so [`Message::Batch`] is excluded.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageEnvelope {
    /// See [`Request`]
    Request(Request),

    /// See [`Response`]
    Response(Response),

    /// See [`Notification`]
    Notification(Notification),
}

/// Represents a non-empty JSON-RPC 2.0 batch.
///
/// A batch is a JSON array of [`MessageEnvelope`] items sent in a single
/// transport write. The server processes all requests in parallel and
/// replies with a single batch response array.
///
/// # Invariant
/// A batch must contain at least one item. Constructing or deserializing
/// an empty batch returns an error.
#[derive(Debug, Clone)]
pub struct MessageBatch {
    /// Per-batch correlation identifier. Never sent over the wire.
    ///
    /// Auto-generated as a UUID on construction or deserialization.
    /// Combined with `session_id` in [`MessageBatch::full_id`] to give the
    /// HTTP transport a unique key for routing the response back to the
    /// correct waiting HTTP handler.
    pub(crate) id: RequestId,

    /// MCP session this batch belongs to. Never sent over the wire.
    ///
    /// Set by the HTTP transport layer, same as for [`Request`] and
    /// [`Notification`].
    pub(crate) session_id: Option<uuid::Uuid>,

    /// HTTP headers from the originating request. Never sent over the wire.
    ///
    /// Copied onto each inner [`Request`] in `execute_batch` so that
    /// middleware (e.g. auth checks) sees the original headers.
    #[cfg(feature = "http-server")]
    pub(crate) headers: HeaderMap,

    /// JWT claims decoded from the originating request. Never sent over the wire.
    ///
    /// Copied onto each inner [`Request`] in `execute_batch` so that
    /// role/permission guards work correctly for batched HTTP calls.
    #[cfg(feature = "http-server")]
    pub(crate) claims: Option<Box<DefaultClaims>>,

    items: Vec<MessageEnvelope>,
}

impl MessageBatch {
    /// Constructs a new [`MessageBatch`] with a freshly generated correlation ID.
    ///
    /// # Errors
    /// Returns [`crate::error::Error`] if `items` is empty.
    pub fn new(items: Vec<MessageEnvelope>) -> Result<Self, crate::error::Error> {
        if items.is_empty() {
            return Err(crate::error::Error::new(
                crate::error::ErrorCode::InvalidRequest,
                "batch must not be empty",
            ));
        }
        Ok(Self {
            id: RequestId::Uuid(uuid::Uuid::new_v4()),
            session_id: None,
            #[cfg(feature = "http-server")]
            headers: HeaderMap::with_capacity(8),
            #[cfg(feature = "http-server")]
            claims: None,
            items,
        })
    }

    /// Returns the full correlation key `(session_id/)id`, matching the
    /// pattern used by [`Request`] and [`Notification`].
    pub(crate) fn full_id(&self) -> RequestId {
        let id = self.id.clone();
        if let Some(session_id) = self.session_id {
            id.concat(RequestId::Uuid(session_id))
        } else {
            id
        }
    }

    /// Returns the number of items in the batch.
    #[inline]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Returns `true` if the batch has no items.
    ///
    /// Note: a [`MessageBatch`] can never be empty after successful construction,
    /// but this method is provided for API completeness.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Returns an iterator over the batch items.
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = &MessageEnvelope> {
        self.items.iter()
    }

    /// Returns `true` if the batch contains at least one [`MessageEnvelope::Request`].
    ///
    /// Used by the HTTP transport to decide whether a pending response slot
    /// must be allocated: a notification-only batch produces no response.
    #[inline]
    #[cfg(any(feature = "http-server", feature = "http-client"))]
    pub(crate) fn has_requests(&self) -> bool {
        self.items.iter().any(|e| matches!(e, MessageEnvelope::Request(_)))
    }
}

impl IntoIterator for MessageBatch {
    type Item = MessageEnvelope;
    type IntoIter = std::vec::IntoIter<MessageEnvelope>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

impl Serialize for MessageBatch {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // `id` and `session_id` are internal — only the items are sent over the wire.
        self.items.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MessageBatch {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Parse as raw JSON values first so that a single malformed element
        // does not discard the entire batch (JSON-RPC §6 requires per-item
        // Invalid Request responses, not a top-level failure).
        let raw = Vec::<serde_json::Value>::deserialize(deserializer)?;
        if raw.is_empty() {
            return Err(serde::de::Error::custom("JSON-RPC batch array must not be empty"));
        }

        let items: Vec<MessageEnvelope> = raw
            .into_iter()
            .filter_map(|value| {
                // Extract the id first (while we still own `value`) so that
                // parse failures can produce a typed Invalid Request response.
                let id = value
                    .get("id")
                    .and_then(|v| serde_json::from_value::<RequestId>(v.clone()).ok());
                match serde_json::from_value::<MessageEnvelope>(value) {
                    Ok(envelope) => Some(envelope),
                    Err(_) => id.map(|req_id| {
                        // Per spec: malformed items with an identifiable id
                        // MUST receive an Invalid Request error response.
                        MessageEnvelope::Response(Response::error(
                            req_id,
                            crate::error::Error::new(
                                crate::error::ErrorCode::InvalidRequest,
                                "Invalid Request",
                            ),
                        ))
                    }),
                    // Items without an id and unparseable are silently skipped.
                }
            })
            .collect();

        if items.is_empty() {
            return Err(serde::de::Error::custom("JSON-RPC batch array must not be empty"));
        }
        Ok(Self {
            id: RequestId::Uuid(uuid::Uuid::new_v4()),
            session_id: None,
            #[cfg(feature = "http-server")]
            headers: HeaderMap::with_capacity(8),
            #[cfg(feature = "http-server")]
            claims: None,
            items,
        })
    }
}

/// Parameters for an initialization request sent to the server.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeRequestParams {
    /// The version of the Model Context Protocol that the client is to use.
    #[serde(rename = "protocolVersion")]
    pub protocol_ver: String,
    
    /// The client's capabilities.
    pub capabilities: Option<ClientCapabilities>,
    
    /// Information about the client implementation.
    #[serde(rename = "clientInfo")]
    pub client_info: Option<Implementation>,
}

/// Result of the initialization request sent to the server.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeResult {
    /// The version of the Model Context Protocol that the server is to use.
    #[serde(rename = "protocolVersion")]
    pub protocol_ver: String,
    
    /// The server's capabilities.
    pub capabilities: ServerCapabilities,
    
    /// Information about the server implementation.
    #[serde(rename = "serverInfo")]
    pub server_info : Implementation,
    
    /// Optional instructions for using the server and its features.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>
}

/// Describes the name and version of an MCP implementation.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Implementation {
    /// Name of the implementation.
    pub name: String,
    
    /// Version of the implementation.
    pub version: String,
    
    /// Optional set of sized icons that the client can display in a user interface.
    /// 
    /// Clients that support rendering icons **MUST** support at least the following MIME types:
    /// - `image/png` - PNG images (safe, universal compatibility)
    /// - `image/jpeg` (and `image/jpg`) - JPEG images (safe, universal compatibility)
    /// 
    /// Clients that support rendering icons **SHOULD** also support:
    /// - `image/svg+xml` - SVG images (scalable but requires security precautions)
    /// - `image/webp` - WebP images (modern, efficient format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<Icon>>,
}

/// Represents the type of role in the conversation.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Corresponds to the user in the conversation.
    User,
    /// Corresponds to the AI in the conversation.
    Assistant
}

/// Represents annotations that can be attached to content.
/// The client can use annotations to inform how objects are used or displayed
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Annotations {
    /// Describes who the intended customer of this object or data is.
    audience: Vec<Role>,
    
    /// The moment the resource was last modified, as an ISO 8601 formatted string.
    /// 
    /// Should be an ISO 8601 formatted string (e.g., **"2025-01-12T15:00:58Z"**).
    /// 
    /// **Examples:** last activity timestamp in an open file, timestamp when the resource
    /// was attached, etc.
    #[serde(rename = "lastModified", skip_serializing_if = "Option::is_none")]
    last_modified: Option<DateTime<Utc>>,
    
    /// Describes how important this data is for operating the server (0 to 1).
    /// 
    /// A value of 1 means **most important** and indicates that the data is
    /// effectively required, while 0 means **least important** and indicates that
    /// the data is entirely optional.
    priority: f32
}

impl Display for Role {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { 
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
        }
    }
}

impl From<&str> for Role {
    #[inline]
    fn from(role: &str) -> Self {
        match role { 
            "user" => Self::User,
            "assistant" => Self::Assistant,
            _ => Self::User
        }
    }
}

impl From<String> for Role {
    #[inline]
    fn from(role: String) -> Self {
        match role.as_str() {
            "user" => Self::User,
            "assistant" => Self::Assistant,
            _ => Self::User
        }
    }
}

impl Default for Implementation {
    fn default() -> Self {
        Self {
            name: SDK_NAME.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            icons: None,
        }
    }
}

impl IntoResponse for InitializeResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        match serde_json::to_value(self) {
            Ok(v) => Response::success(req_id, v),
            Err(err) => Response::error(req_id, err.into())
        }
    }
}

#[cfg(feature = "server")]
impl FromHandlerParams for InitializeRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

impl From<MessageEnvelope> for Message {
    fn from(envelope: MessageEnvelope) -> Self {
        match envelope {
            MessageEnvelope::Request(r) => Message::Request(r),
            MessageEnvelope::Response(r) => Message::Response(r),
            MessageEnvelope::Notification(n) => Message::Notification(n),
        }
    }
}

impl Message {
    /// Returns `true` is the current message is [`Request`]
    #[inline]
    pub fn is_request(&self) -> bool {
        matches!(self, Message::Request(_))
    }

    /// Returns `true` is the current message is [`Response`]
    #[inline]
    pub fn is_response(&self) -> bool {
        matches!(self, Message::Response(_))
    }

    /// Returns `true` is the current message is [`Notification`]
    #[inline]
    pub fn is_notification(&self) -> bool {
        matches!(self, Message::Notification(_))
    }

    /// Returns `true` if this message is a [`MessageBatch`]
    #[inline]
    pub fn is_batch(&self) -> bool {
        matches!(self, Message::Batch(_))
    }
    
    /// Returns [`Message`] ID
    #[inline]
    pub fn id(&self) -> RequestId {
        match self {
            Message::Request(req) => req.id(),
            Message::Response(resp) => resp.id().clone(),
            Message::Notification(_) | Message::Batch(_) => RequestId::default()
        }
    }

    /// Returns the full id (session_id?/message_id)
    pub fn full_id(&self) -> RequestId {
        match self {
            Message::Request(req) => req.full_id(),
            Message::Response(resp) => resp.full_id(),
            Message::Notification(notification) => notification.full_id(),
            Message::Batch(batch) => batch.full_id(),
        }
    }

    /// Returns MCP Session ID
    #[inline]
    pub fn session_id(&self) -> Option<&uuid::Uuid> {
        match self {
            Message::Request(req) => req.session_id.as_ref(),
            Message::Response(resp) => resp.session_id(),
            Message::Notification(notification) => notification.session_id.as_ref(),
            Message::Batch(batch) => batch.session_id.as_ref(),
        }
    }

    /// Sets MCP Session ID
    pub fn set_session_id(mut self, id: uuid::Uuid) -> Self {
        match self {
            Message::Request(ref mut req) => req.session_id = Some(id),
            Message::Notification(ref mut notification) => notification.session_id = Some(id),
            Message::Response(resp) => self = Message::Response(resp.set_session_id(id)),
            Message::Batch(ref mut batch) => batch.session_id = Some(id),
        }
        self
    }
    
    /// Sets HTTP headers for [`Request`], [`Response`], or [`MessageBatch`] message
    #[cfg(feature = "http-server")]
    pub fn set_headers(mut self, headers: HeaderMap) -> Self {
        match self {
            Message::Request(ref mut req) => req.headers = headers,
            Message::Response(resp) => self = Message::Response(resp.set_headers(headers)),
            Message::Batch(ref mut batch) => batch.headers = headers,
            _ => ()
        }
        self
    }

    /// Sets Authentication and Authorization claims for [`Request`] or [`MessageBatch`] message
    #[cfg(feature = "http-server")]
    pub(crate) fn set_claims(mut self, claims: DefaultClaims) -> Self {
        match self {
            Message::Request(ref mut req) => req.claims = Some(Box::new(claims)),
            Message::Batch(ref mut batch) => batch.claims = Some(Box::new(claims)),
            _ => ()
        }
        self
    }
}

impl Annotations {
    /// Deserializes a new [`Annotations`] from a JSON string
    #[inline]
    pub fn from_json_str(json: &str) -> Self {
        serde_json::from_str(json)
            .expect("Annotations: Incorrect JSON string provided")
    }
    
    /// Adds audience
    pub fn with_audience<T: Into<Role>>(mut self, role: T) -> Self {
        self.audience.push(role.into());
        self
    }
    
    /// Sets the priority
    pub fn with_priority(mut self, priority: f32) -> Self {
        self.priority = priority;
        self
    }
    
    /// Sets the moment the object was last modified
    pub fn with_last_modified(mut self, last_modified: DateTime<Utc>) -> Self {
        self.last_modified = Some(last_modified);
        self
    }
}

impl Implementation {
    /// Specifies icons
    #[inline]
    pub fn with_icons(mut self, icons: impl IntoIterator<Item = Icon>) -> Self {
        self.icons = Some(icons.into_iter().collect());
        self
    }
}

#[cfg(feature = "server")]
impl InitializeResult {
    pub(crate) fn new(options: &McpOptions) -> Self {
        Self {
            protocol_ver: options.protocol_ver().into(),
            capabilities: ServerCapabilities {
                tools: options.tools_capability(),
                resources: options.resources_capability(),
                prompts: options.prompts_capability(),
                logging: Some(LoggingCapability::default()),
                completions: Some(CompletionsCapability::default()),
                #[cfg(feature = "tasks")]
                tasks: options.tasks_capability(),
                experimental: None
            },
            server_info: options.implementation.clone(),
            instructions: None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_envelope_deserializes_request() {
        let json = r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":null}"#;
        let envelope: MessageEnvelope = serde_json::from_str(json).unwrap();
        assert!(matches!(envelope, MessageEnvelope::Request(_)));
    }

    #[test]
    fn message_batch_rejects_empty_vec() {
        let err = MessageBatch::new(vec![]);
        assert!(err.is_err());
    }

    #[test]
    fn message_batch_rejects_empty_json_array() {
        let err: Result<MessageBatch, _> = serde_json::from_str("[]");
        assert!(err.is_err());
    }

    #[test]
    fn message_batch_accepts_non_empty() {
        let json = r#"[{"jsonrpc":"2.0","id":1,"method":"ping","params":null}]"#;
        let batch: MessageBatch = serde_json::from_str(json).unwrap();
        assert_eq!(batch.len(), 1);
    }

    #[test]
    fn message_deserializes_batch() {
        let json = r#"[{"jsonrpc":"2.0","id":1,"method":"ping","params":null}]"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, Message::Batch(_)));
    }

    #[test]
    fn message_batch_skips_malformed_item_without_id() {
        // A malformed item with no "id" field is silently dropped.
        let json = r#"[
            {"jsonrpc":"2.0","id":1,"method":"ping","params":null},
            {"not":"valid json-rpc"}
        ]"#;
        let batch: MessageBatch = serde_json::from_str(json).unwrap();
        assert_eq!(batch.len(), 1);
        assert!(matches!(batch.iter().next(), Some(MessageEnvelope::Request(_))));
    }

    #[test]
    fn message_batch_produces_error_response_for_malformed_item_with_id() {
        // A malformed item that carries an "id" yields an Invalid Request response.
        let json = r#"[
            {"jsonrpc":"2.0","id":1,"method":"ping","params":null},
            {"jsonrpc":"2.0","id":2,"params":"not-an-object-and-no-method"}
        ]"#;
        let batch: MessageBatch = serde_json::from_str(json).unwrap();
        assert_eq!(batch.len(), 2);
        let mut iter = batch.iter();
        assert!(matches!(iter.next(), Some(MessageEnvelope::Request(_))));
        let second = iter.next().expect("second item should be present");
        let MessageEnvelope::Response(resp) = second else {
            panic!("expected error response for malformed item, got {second:?}");
        };
        assert!(matches!(resp, Response::Err(_)), "expected error response");
    }

    #[test]
    fn message_batch_rejects_all_malformed_items_without_ids() {
        // If every item is malformed AND none carry an "id", the batch itself
        // fails to deserialize (nothing valid to process).
        let json = r#"[{"not":"valid"},{"also":"not valid"}]"#;
        let err: Result<MessageBatch, _> = serde_json::from_str(json);
        assert!(err.is_err());
    }
}
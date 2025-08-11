use std::fmt::Display;
use serde::{Deserialize, Serialize};
use crate::SDK_NAME;
use crate::types::notification::Notification;

#[cfg(feature = "server")]
use crate::{
    app::handler::{FromHandlerParams, HandlerParams},
    types::request::FromRequest,
    options::McpOptions,
    error::Error,
};

#[cfg(feature = "http-server")]
use {
    crate::auth::DefaultClaims,
    volga::headers::HeaderMap
};

pub use helpers::{Json, Meta, PropertyType};
pub use request::{RequestId, Request};
pub use response::{IntoResponse, Response};
pub use reference::Reference;
pub use completion::{Completion, CompleteRequestParams, Argument, CompleteResult};
pub use content::Content;
pub use cursor::{Cursor, Page, Pagination};
pub use capabilities::{
    ClientCapabilities, 
    ServerCapabilities, 
    ToolsCapability, 
    ResourcesCapability,
    PromptsCapability,
    LoggingCapability,
    CompletionsCapability
};
pub use tool::{
    ListToolsRequestParams,
    CallToolRequestParams,
    CallToolResponse,
    Tool, 
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

#[cfg(feature = "server")]
pub use prompt::PromptHandler;

pub use root::Root;
pub use progress::ProgressToken;

pub mod request;
pub mod response;
pub mod capabilities;
pub mod tool;
pub mod resource;
pub mod prompt;
pub mod completion;
pub mod content;
pub mod reference;
pub mod notification;
pub mod cursor;
pub mod root;
pub mod sampling;
pub mod progress;
pub(crate) mod helpers;

pub(super) const JSONRPC_VERSION: &str = "2.0";

/// Represents a JSON RPC message that could be either [`Request`] or [`Response`] or [`Notification`]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    /// See [`Request`]
    Request(Request),

    /// See [`Response`]
    Response(Response),

    /// See [`Notification`]
    Notification(Notification),
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
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Annotations {
    /// Describes who the intended customer of this object or data is.
    audience: Vec<Role>,
    
    /// Describes how important this data is for operating the server (0 to 1).
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
            version: env!("CARGO_PKG_VERSION").into()
        }
    }
}

impl IntoResponse for InitializeResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        Response::success(req_id, serde_json::to_value(self).unwrap())
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

impl Message {
    /// Returns [`Message`] ID
    #[inline]
    pub fn id(&self) -> RequestId {
        match self { 
            Message::Request(req) => req.id(),
            Message::Response(resp) => resp.id().clone(),
            Message::Notification(_) => RequestId::default()
        }    
    }

    /// Returns the full id (session_id?/message_id)
    pub fn full_id(&self) -> RequestId {
        match self {
            Message::Request(req) => req.full_id(),
            Message::Response(resp) => resp.full_id(),
            Message::Notification(notification) => notification.full_id()
        }
    }
    
    /// Returns MCP Session ID
    #[inline]
    pub fn session_id(&self) -> Option<&uuid::Uuid> {
        match self { 
            Message::Request(req) => req.session_id.as_ref(),
            Message::Response(resp) => resp.session_id(),
            Message::Notification(notification) => notification.session_id.as_ref()
        }
    }
    
    /// Sets MCP Session ID
    pub fn set_session_id(mut self, id: uuid::Uuid) -> Self {
        match self { 
            Message::Request(ref mut req) => req.session_id = Some(id),
            Message::Notification(ref mut notification) => notification.session_id = Some(id),
            Message::Response(resp) => self = Message::Response(resp.set_session_id(id)),
        }
        self
    }
    
    /// Sets HTTP headers for [`Request`] or [`Response`] message
    #[cfg(feature = "http-server")]
    pub fn set_headers(mut self, headers: HeaderMap) -> Self {
        match self { 
            Message::Request(ref mut req) => req.headers = headers,
            Message::Response(resp) => self = Message::Response(resp.set_headers(headers)),
            _ => ()
        }
        self
    }

    /// Sets Authentication and Authorization claims for [`Request`] message
    #[cfg(feature = "http-server")]
    pub(crate) fn set_claims(mut self, claims: DefaultClaims) -> Self {
        if let Message::Request(ref mut req) = self {
            req.claims = Some(Box::new(claims));
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
    pub fn add_audience<T: Into<Role>>(mut self, role: T) -> Self {
        self.audience.push(role.into());
        self
    }
    
    /// Sets the priority
    pub fn set_priority(mut self, priority: f32) -> Self {
        self.priority = priority;
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
                experimental: None
            },
            server_info: options.implementation.clone(),
            instructions: None
        }
    }
}

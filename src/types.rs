use std::fmt::Display;
use serde::{Deserialize, Serialize};
use crate::{PROTOCOL_VERSION, SERVER_NAME};
use crate::options::McpOptions;

pub use helpers::{Json, PropertyType};
pub use request::{RequestId, Request};
pub use response::{IntoResponse, Response};
pub use reference::Reference;
pub use completion::{Completion, CompleteRequestParams, Argument, CompleteResult};
pub use content::Content;
pub use capabilities::{
    ClientCapabilities, 
    ServerCapabilities, 
    ToolsCapability, 
    ResourcesCapability
};
pub use tool::{
    ListToolsRequestParams,
    CallToolRequestParams,
    CallToolResponse,
    Tool, 
    ToolHandler, 
    ListToolsResult
};
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
    PromptHandler,
};
use crate::app::handler::{FromHandlerParams, HandlerParams};
use crate::error::Error;
use crate::types::capabilities::PromptsCapability;
use crate::types::request::FromRequest;

pub mod request;
pub mod response;
pub mod capabilities;
pub mod tool;
pub mod resource;
pub mod prompt;
pub mod completion;
pub mod content;
pub mod reference;
pub(crate) mod helpers;

pub(super) const JSONRPC_VERSION: &str = "2.0";

/// Parameters for an initialization request sent to the server.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Debug, Clone, Deserialize)]
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
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Debug, Clone, Serialize)]
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
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Implementation {
    /// Name of the implementation.
    pub name: String,
    
    /// Version of the implementation.
    pub version: String,
}

/// Represents the type of role in the conversation.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Corresponds to the user in the conversation.
    User,
    /// Corresponds to the AI in the conversation.
    Assistant
}

/// Represents annotations that can be attached to content.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
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
            name: SERVER_NAME.into(),
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

impl FromHandlerParams for InitializeRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

impl Annotations {
    /// Adds audience
    pub fn add_audience<T: Into<Role>>(mut self, role: T) -> Self {
        self.audience.push(role.into());
        self
    }
    
    /// Sets priority
    pub fn set_priority(mut self, priority: f32) -> Self {
        self.priority = priority;
        self
    }
}

impl InitializeResult {
    pub(crate) fn new(options: &McpOptions) -> Self {
        Self {
            protocol_ver: PROTOCOL_VERSION.into(),
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability {
                    list_changed: true
                }),
                resources: Some(ResourcesCapability {
                    list_changed: true,
                    subscribe: false
                }),
                prompts: Some(PromptsCapability {
                    list_changed: true,
                }),
            },
            server_info: options.implementation.clone(),
            instructions: None
        }
    }
}

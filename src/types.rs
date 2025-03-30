use serde::{Deserialize, Serialize};
use crate::{PROTOCOL_VERSION, SERVER_NAME};
use crate::options::McpOptions;

pub use helpers::Json;
pub use request::{RequestId, Request};
pub use response::{IntoResponse, Response};
pub use completion::{Completion, CompleteResult};
pub use capabilities::{ClientCapabilities, ServerCapabilities, ToolsCapability};
pub use tool::{CallToolRequestParams, Tool, ToolHandler, ListToolsResult};
pub use resource::{
    ListResourcesResult, 
    Resource, 
    ResourceContents, 
    ReadResourceResult, 
    ReadResourceRequestParams
};
pub use prompt::{
    ListPromptsResult,
    Prompt,
    GetPromptRequestParams,
    GetPromptResult,
    PromptArgument,
    PromptMessage,
    Role
};

pub mod request;
pub mod response;
pub mod capabilities;
pub mod tool;
pub mod resource;
pub mod prompt;
pub mod completion;
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

impl InitializeResult {
    pub(crate) fn new(options: &McpOptions) -> Self {
        Self {
            protocol_ver: PROTOCOL_VERSION.into(),
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability {
                    list_changed: true
                }),
                prompts: None,
                resources: None
            },
            server_info: options.implementation.clone(),
            instructions: None
        }
    }
}

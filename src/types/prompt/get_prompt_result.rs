//! Types and utils for prompt request results

use serde::Serialize;
use crate::types::{Content, IntoResponse, RequestId, Response, Role};

/// The server's response to a prompts/get request from the client.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Default, Serialize)]
pub struct GetPromptResult {
    /// An optional description for the prompt.
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,

    /// The prompt or prompt template that the server offers.
    pub messages: Vec<PromptMessage>,
}

/// Describes a message returned as part of a prompt.
///
/// This is similar to `SamplingMessage`, but also supports the embedding of 
/// resources from the MCP server.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Serialize)]
pub struct PromptMessage {
    /// The content of the message. Any of TextContent, ImageContent, EmbeddedResource.
    pub content: Content,

    /// The role of the message ("user" or "assistant").
    pub role: Role,
}

impl IntoResponse for GetPromptResult {
    fn into_response(self, req_id: RequestId) -> Response {
        Response::success(req_id, serde_json::to_value(self).unwrap())
    }
}
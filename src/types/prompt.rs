//! Represents an MCP prompt

use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use crate::types::{IntoResponse, RequestId, Response, Content};

/// A prompt or prompt template that the server offers.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Serialize)]
pub struct Prompt {
    /// The name of the prompt or prompt template.
    pub name: String,

    /// An optional description of what this prompt provides
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,

    /// A list of arguments to use for templating the prompt.
    #[serde(rename = "arguments", skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<PromptArgument>>,
}

/// Describes an argument that a prompt can accept.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Serialize)]
pub struct PromptArgument {
    /// The name of the argument.
    pub name: String,

    /// A human-readable description of the argument.
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,

    /// Whether this argument must be provided.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

/// Used by the client to get a prompt provided by the server.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Deserialize)]
pub struct GetPromptRequestParams {
    /// The name of the prompt or prompt template.
    pub name: String,
    
    /// Arguments to use for templating the prompt.
    #[serde(rename = "arguments", skip_serializing_if = "Option::is_none")]
    pub args: Option<HashMap<String, serde_json::Value>>,
}

/// The server's response to a prompts/get request from the client.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Deserialize)]
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
#[derive(Deserialize)]
pub struct PromptMessage {
    /// The content of the message. Any of TextContent, ImageContent, EmbeddedResource.
    pub content: Content,
    
    /// The role of the message ("user" or "assistant").
    pub role: Role,
}

/// Represents the type of role in the conversation.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Deserialize)]
pub enum Role {
    /// Corresponds to the user in the conversation.
    User,
    /// Corresponds to the AI in the conversation.
    Assistant
}

/// The server's response to a prompts/list request from the client.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Default, Serialize)]
pub struct ListPromptsResult<'a> {
    /// A list of prompts or prompt templates that the server offers.
    pub prompts: Vec<&'a Prompt>,
}

impl IntoResponse for ListPromptsResult<'_> {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        Response::success(req_id, serde_json::to_value(self).unwrap())
    }
}

impl<'a> From<Vec<&'a Prompt>> for ListPromptsResult<'a> {
    #[inline]
    fn from(prompts: Vec<&'a Prompt>) -> Self {
        Self { prompts }
    }
}

impl ListPromptsResult<'_> {
    /// Create a new [`ListPromptsResult`]
    #[inline]
    pub fn new() -> Self {
        Default::default()
    }
}

impl Prompt {
    /// Creates a new [`Prompt`]
    #[inline]
    pub fn new(name: &str) -> Self {
        // TODO: impl
        Self { name: name.into(), descr: None, args: None }
    }
}
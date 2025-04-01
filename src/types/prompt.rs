//! Represents an MCP prompt

use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use crate::types::{IntoResponse, RequestId, Response};

pub use get_prompt_result::{GetPromptResult, PromptMessage};

pub mod get_prompt_result;

/// A prompt or prompt template that the server offers.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Clone, Serialize)]
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
#[derive(Clone, Serialize)]
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

/// Sent from the client to request a list of prompts and prompt templates the server has.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Deserialize)]
pub struct ListPromptsRequestParams {
    /// An opaque token representing the current pagination position.
    /// If provided, the server should return results starting after this cursor.
    pub cursor: Option<String>,
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

/// The server's response to a prompts/list request from the client.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Default, Serialize)]
pub struct ListPromptsResult {
    /// A list of prompts or prompt templates that the server offers.
    pub prompts: Vec<Prompt>,
}

impl IntoResponse for ListPromptsResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        Response::success(req_id, serde_json::to_value(self).unwrap())
    }
}

impl From<Vec<Prompt>> for ListPromptsResult {
    #[inline]
    fn from(prompts: Vec<Prompt>) -> Self {
        Self { prompts }
    }
}

impl ListPromptsResult {
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
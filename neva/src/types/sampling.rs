//! Utilities for Sampling

use serde::{Serialize, Deserialize};
use crate::types::{Content, Role, IntoResponse, RequestId, Response};
#[cfg(feature = "client")]
use std::{pin::Pin, sync::Arc, future::Future};

const DEFAULT_MESSAGE_MAX_TOKENS: i32 = 512;

/// List of commands for Roots
pub mod commands {
    pub const CREATE: &str = "sampling/createMessage";
}

/// Represents a message issued to or received from an LLM API within the Model Context Protocol.
/// 
/// > **Note:** A [`SamplingMessage`] encapsulates content sent to or received from AI models in the Model Context Protocol.
/// > Each message has a specific role [`Role::User`] or [`Role::Assistant`] and contains content which can be text or images.
/// > 
/// > [`SamplingMessage`] objects are typically used in collections within [`CreateMessageRequestParams`]
/// > to represent prompts or queries for LLM sampling. They form the core data structure for text generation requests
/// > within the Model Context Protocol.
/// > 
/// > While similar, to [`PromptMessage`], the [`SamplingMessage`] is focused on direct LLM sampling
/// > operations rather than the enhanced resource embedding capabilities provided by [`PromptMessage`].
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Serialize, Deserialize)]
pub struct SamplingMessage {
    /// The role of the message sender, indicating whether it's from a _user_ or an _assistant_.
    pub role: Role,
    
    /// The content of the message.
    pub content: Content
}

/// Represents the parameters used with a _"sampling/createMessage"_ 
/// request from a server to sample an LLM via the client.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Serialize, Deserialize)]
pub struct CreateMessageRequestParams {
    /// The messages requested by the server to be included in the prompt.
    pub messages: Vec<SamplingMessage>,
    
    /// The maximum number of tokens to generate in the LLM response, as requested by the server.
    ///
    /// > **Note:** A token is generally a word or part of a word in the text. Setting this value helps control 
    /// > response length and computation time. The client may choose to sample fewer tokens than requested.
    #[serde(rename = "maxTokens")]
    pub max_tokens: i32,
    
    /// Represents an indication as to which server contexts should be included in the prompt.
    /// 
    /// > **Note:** The client may ignore this request.
    #[serde(rename = "includeContext", skip_serializing_if = "Option::is_none")]
    pub include_context: Option<ContextInclusion>,
    
    /// An optional metadata to pass through to the LLM provider.
    ///
    /// > **Note:** The format of this metadata is provider-specific and can include model-specific settings or
    /// > configuration that isn't covered by standard parameters. This allows for passing custom parameters 
    /// > that are specific to certain AI models or providers.
    #[serde(rename = "metadata", skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
    
    /// Represents the server's preferences for which model to select.
    ///
    /// > **Note:** The client may ignore these preferences.
    /// > 
    /// > These preferences help the client make an appropriate model selection based on the server's priorities
    /// > for cost, speed, intelligence, and specific model hints.
    /// > 
    /// > When multiple dimensions are specified (cost, speed, intelligence), the client should balance these
    /// > based on their relative values. If specific model hints are provided, the client should evaluate them
    /// > in order and prioritize them over numeric priorities.
    #[serde(rename = "modelPreferences", skip_serializing_if = "Option::is_none")]
    pub model_pref: Option<ModelPreferences>,

    /// Represents an optional system prompt the server wants to use for sampling.
    ///
    /// > **Note:** The client may modify or omit this prompt.
    #[serde(rename = "systemPrompt", skip_serializing_if = "Option::is_none")]
    pub sys_prompt: Option<String>,

    /// Represents the temperature to use for sampling, as requested by the server.
    #[serde(rename = "temperature", skip_serializing_if = "Option::is_none")]
    pub temp: Option<f32>,
    
    /// Represents optional sequences of characters that signal the LLM to stop generating text when encountered.
    ///
    /// > **Note:** When the model generates any of these sequences during sampling, text generation stops immediately,
    /// > even if the maximum token limit hasn't been reached. This is useful for controlling generation 
    /// > endings or preventing the model from continuing beyond certain points.
    /// > 
    /// > Stop sequences are typically case-sensitive, and typically the LLM will only stop generation when a produced
    /// > sequence exactly matches one of the provided sequences. Common uses include ending markers like _"END"_, punctuation
    /// > like _"."_, or special delimiter sequences like _"###"_.
    #[serde(rename = "stopSequences", skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
}

/// Specifies the context inclusion options for a request in the Model Context Protocol (MCP).
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Serialize, Deserialize)]
pub enum ContextInclusion {
    /// Indicates that no context should be included.
    #[serde(rename = "none")]
    None,
    
    /// Indicates that context from the server that sent the request should be included.
    #[serde(rename = "thisServer")]
    ThisServer,
    
    /// Indicates that context from all servers that the client is connected to should be included.
    #[serde(rename = "allServers")]
    AllServers
}

/// Represents a server's preferences for model selection, requested of the client during sampling.
///
/// > **Note:** Because LLMs can vary along multiple dimensions, choosing the _best_ model is
/// > rarely straightforward.  Different models excel in different areas—some are
/// > faster but less capable, others are more capable but more expensive, and so
/// > on. This struct allows servers to express their priorities across multiple
/// > dimensions to help clients make an appropriate selection for their use case.
/// > 
/// > These preferences are always advisory. The client may ignore them. It is also
/// > up to the client to decide how to interpret these preferences and how to
/// > balance them against other considerations.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Serialize, Deserialize)]
pub struct ModelPreferences {
    /// Represents how much to prioritize cost when selecting a model.
    /// 
    /// > **Note:** A value of _0_ means cost is not important, 
    /// > while a value of _1_ means cost is the most important factor.
    #[serde(rename = "costPriority", skip_serializing_if = "Option::is_none")]
    pub cost_priority: Option<f32>,

    /// Optional hints to use for model selection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hints: Option<Vec<ModelHint>>,

    /// Represents how much to prioritize sampling speed (latency) when selecting a model.
    /// 
    /// > **Note:** A value of _0_ means speed is not important, 
    /// > while a value of _1_ means speed is the most important factor.
    #[serde(rename = "speedPriority", skip_serializing_if = "Option::is_none")]
    pub speed_priority: Option<f32>,

    /// Represents how much to prioritize intelligence and capabilities when selecting a model.
    /// 
    /// > **Note:** A value of _0_ means intelligence is not important, 
    /// > while a value of _1_ means intelligence is the most important factor.
    #[serde(rename = "intelligencePriority", skip_serializing_if = "Option::is_none")]
    pub intelligence_priority: Option<f32>,
}

/// Provides hints to use for model selection.
///
/// > **Note:** When multiple hints are specified in [`ModelPreferences`], they are evaluated in order,
/// > with the first match taking precedence. 
/// > 
/// > Clients should prioritize these hints over numeric priorities.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Serialize, Deserialize)]
pub struct ModelHint {
    /// A hint for a model name.
    /// 
    /// > **Note:** The specified string can be a partial or full model name. Clients may also 
    /// > map hints to equivalent models from different providers. Clients make the final model
    /// > selection based on these preferences and their available models.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Represents a client's response to a _"sampling/createMessage"_ from the server.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Serialize, Deserialize)]
pub struct CreateMessageResult {
    /// Role of the user who generated the message.
    pub role: Role,
    
    /// Content of the message.
    pub content: Content,
    
    /// Name of the model that generated the message.
    ///
    /// > **Note:** This should contain the specific model identifier such as _"claude-3-5-sonnet-20241022"_ or _"o3-mini"_.
    /// > 
    /// > This property allows the server to know which model was used to generate the response,
    /// > enabling appropriate handling based on the model's capabilities and characteristics.
    pub model: String,

    /// Reason why message generation (sampling) stopped, if known.
    /// 
    /// ### Common values include:
    /// * `endTurn` The model naturally completed its response.
    /// * `maxTokens` The response was truncated due to reaching token limits.
    /// * `stopSequence` A specific stop sequence was encountered during generation.
    #[serde(rename = "stopReason", skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
}

impl Default for CreateMessageRequestParams {
    #[inline]
    fn default() -> Self {
        Self {
            max_tokens: DEFAULT_MESSAGE_MAX_TOKENS,
            messages: Vec::new(),
            sys_prompt: None,
            include_context: None,
            meta: None,
            model_pref: None,
            temp: None,
            stop_sequences: None
        }
    }
}

impl IntoResponse for CreateMessageResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        Response::success(req_id, serde_json::to_value(self).unwrap())
    }
}

impl From<&str> for SamplingMessage {
    #[inline]
    fn from(s: &str) -> Self {
        Self::new(Role::User, Content::text(s))
    }
}

impl SamplingMessage {
    /// Creates a new [`SamplingMessage`]
    pub fn new<T: Into<Content>>(role: Role, content: T) -> Self {
        Self { role, content: content.into() }
    }
}

impl CreateMessageRequestParams {
    /// Creates a new empty params
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Creates params for a single message request
    pub fn message(message: &str, sys_prompt: &str) -> Self {
        let mut params = Self::default();
        params.messages.push(message.into());
        params.sys_prompt = Some(sys_prompt.into());
        params
    }
    
    /// Creates params for many messages request
    pub fn messages<T, I>(messages: I, sys_prompt: &str) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<SamplingMessage>,
    {
        let mut params = Self::default();
        params.messages.extend(messages.into_iter().map(Into::into));
        params.sys_prompt = Some(sys_prompt.into());
        params
    }
}

impl CreateMessageResult {
    /// Creates a new [`CreateMessageResult`]
    pub fn new<T: Into<Content>>(role: Role, model: &str, content: T) -> Self {
        Self {
            stop_reason: None,
            model: model.into(),
            content: content.into(),
            role,
        }
    }
}

/// Represents a dynamic handler for handling sampling requests
#[cfg(feature = "client")]
pub(crate) type SamplingHandler = Arc<
    dyn Fn(CreateMessageRequestParams) -> Pin<
        Box<dyn Future<Output = CreateMessageResult> + Send + 'static>
    > 
    + Send 
    + Sync
>;

//! Utilities for Sampling

use serde::{Serialize, Deserialize};
use crate::types::{Content, Role, IntoResponse, RequestId, Response};
#[cfg(feature = "client")]
use std::{pin::Pin, sync::Arc, future::Future};

const DEFAULT_MESSAGE_MAX_TOKENS: i32 = 512;

/// List of commands for Sampling
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
#[derive(Default, Serialize, Deserialize)]
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
#[derive(Default, Serialize, Deserialize)]
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
    /// > enabling the appropriate handling based on the model's capabilities and characteristics.
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
            messages: Vec::with_capacity(8),
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
        Self::user().with(s)
    }
}

impl From<String> for SamplingMessage {
    #[inline]
    fn from(s: String) -> Self {
        Self::user().with(s)
    }
}

impl From<&str> for ModelHint {
    #[inline]
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for ModelHint {
    #[inline]
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl SamplingMessage {
    /// Creates a new [`SamplingMessage`]
    #[inline]
    pub fn new(role: Role) -> Self {
        Self { role, content: Content::empty() }
    }
    
    /// Creates a new [`SamplingMessage`] with a user role
    pub fn user() -> Self {
        Self::new(Role::User)
    }
    
    /// Creates a new [`SamplingMessage`] with an assistant role
    pub fn assistant() -> Self {
        Self::new(Role::Assistant)
    }
    
    /// Sets the content
    pub fn with<T: Into<Content>>(mut self, content: T) -> Self {
        self.content = content.into();
        self
    }
}

impl ModelPreferences {
    /// Creates a new [`ModelPreferences`]
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Sets the cost priority
    pub fn with_cost_priority(mut self, priority: f32) -> Self {
        self.cost_priority = Some(priority);
        self
    }
    
    /// Sets the speed priority
    pub fn with_speed_priority(mut self, priority: f32) -> Self {
        self.speed_priority = Some(priority);
        self
    }
    
    /// Sets the intelligence priority
    pub fn with_intel_priority(mut self, priority: f32) -> Self {
        self.intelligence_priority = Some(priority);
        self
    }
    
    /// Sets the model hint
    pub fn with_hint(mut self, hint: impl Into<ModelHint>) -> Self {
        self.hints
            .get_or_insert_with(Vec::new)
            .push(hint.into());
        self
    }

    /// Sets the model hints
    pub fn with_hints<T , I>(mut self, hint: T) -> Self
    where 
        T: IntoIterator<Item = I>,
        I: Into<ModelHint>
    {
        self.hints
            .get_or_insert_with(Vec::new)
            .extend(hint.into_iter().map(Into::into));
        self
    }
}

impl ModelHint {
    /// Creates a new [`ModelHint`]
    #[inline]
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: Some(name.into()) }
    }
}

impl CreateMessageRequestParams {
    /// Creates a new empty params
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Creates params for a single message request
    pub fn with_message(mut self, message: impl Into<SamplingMessage>) -> Self {
        self.messages.push(message.into());
        self
    }
    
    /// Creates params for multiple messages request
    pub fn with_messages<T, I>(mut self, messages: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<SamplingMessage>,
    {
        self.messages
            .extend(messages.into_iter().map(Into::into));
        self
    }
    
    pub fn with_sys_prompt(mut self, sys_prompt: impl Into<String>) -> Self {
        self.sys_prompt = Some(sys_prompt.into());
        self
    }
    
    /// Sets the `max_tokens` for this [`CreateMessageRequestParams`]
    pub fn with_max_tokens(mut self, max_tokens: i32) -> Self {
        self.max_tokens = max_tokens;
        self
    }
    
    /// Sets the [`ContextInclusion`] for this [`CreateMessageRequestParams`]
    pub fn with_include_ctx(mut self, inc: ContextInclusion) -> Self {
        self.include_context = Some(inc);
        self
    }

    /// Sets the [`ContextInclusion::None`] for this [`CreateMessageRequestParams`]
    pub fn with_no_ctx(mut self) -> Self {
        self.include_context = Some(ContextInclusion::None);
        self
    }

    /// Sets the [`ContextInclusion::ThisServer`] for this [`CreateMessageRequestParams`]
    pub fn with_this_server(mut self) -> Self {
        self.include_context = Some(ContextInclusion::ThisServer);
        self
    }

    /// Sets the [`ContextInclusion::AllServers`] for this [`CreateMessageRequestParams`]
    pub fn with_all_servers(mut self) -> Self {
        self.include_context = Some(ContextInclusion::AllServers);
        self
    }
    
    /// Sets the [`ModelPreferences`] for this [`CreateMessageRequestParams`]
    pub fn with_pref(mut self, pref: ModelPreferences) -> Self {
        self.model_pref = Some(pref);
        self
    }
    
    /// Sets a temperature for this [`CreateMessageRequestParams`]
    pub fn with_temp(mut self, temp: f32) -> Self {
        self.temp = Some(temp);
        self
    }
    
    /// Sets the stop sequences for this [`CreateMessageRequestParams`]
    pub fn with_stop_seq(mut self, stop_sequences: Vec<String>) -> Self {
        self.stop_sequences = Some(stop_sequences);
        self
    }
    
    /// Returns an iterator of text messages
    pub fn text(&self) -> impl Iterator<Item = &Content> {
        self.msg_iter("text")
    }

    /// Returns an iterator of audio messages
    pub fn audio(&self) -> impl Iterator<Item = &Content> {
        self.msg_iter("audio")
    }

    /// Returns an iterator of image messages
    pub fn images(&self) -> impl Iterator<Item = &Content> {
        self.msg_iter("image")
    }
    
    #[inline]
    fn msg_iter(&self, t: &'static str) -> impl Iterator<Item = &Content> {
        self.messages
            .iter()
            .filter_map(move |m| {
                if m.content.r#type == t {
                    Some(&m.content)
                } else {
                    None
                }
            })
    }
}

impl CreateMessageResult {
    /// Creates a new [`CreateMessageResult`]
    #[inline]
    pub fn new(role: Role) -> Self {
        Self {
            stop_reason: None,
            model: String::new(),
            content: Content::empty(),
            role,
        }
    }
    
    /// Creates a new [`CreateMessageResult`] with a user role
    pub fn user() -> Self {
        Self::new(Role::User)
    }
    
    /// Creates a new [`CreateMessageResult`] with an assistant role
    pub fn assistant() -> Self {
        Self::new(Role::Assistant)
    }
    
    /// Sets the stop reason
    pub fn with_stop_reason(mut self, reason: impl Into<String>) -> Self {
        self.stop_reason = Some(reason.into());
        self
    }
    
    /// Sets the model name
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }
    
    /// Sets the content
    pub fn with_content<T: Into<Content>>(mut self, content: T) -> Self {
        self.content = content.into();
        self
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

//! Utilities for Sampling

use serde::{Serialize, Deserialize, Serializer, Deserializer};
use crate::shared::{OneOrMany, IntoArgs};
use crate::types::{
    Tool, ToolUse, ToolResult,
    Content, TextContent, ImageContent, AudioContent,
    ResourceLink, EmbeddedResource,
    PromptMessage, 
    Role, 
    RequestId, 
    Response, 
    IntoResponse
};

#[cfg(feature = "client")]
use std::{pin::Pin, sync::Arc, future::Future};
#[cfg(feature = "tasks")]
use crate::types::TaskMetadata;

const DEFAULT_MESSAGE_MAX_TOKENS: i32 = 512;

/// List of commands for Sampling
pub mod commands {
    /// Command name for sampling
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamplingMessage {
    /// The role of the message sender, indicating whether it's from a _user_ or an _assistant_.
    pub role: Role,
    
    /// The content of the message.
    pub content: OneOrMany<Content>
}

/// Represents the parameters used with a _"sampling/createMessage"_ 
/// request from a server to sample an LLM via the client.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
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

    /// Tools that the model may use during generation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,

    /// Controls how the model uses tools.
    /// 
    /// Default is `{ mode: "auto" }`.
    #[serde(rename = "toolChoice", skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,

    /// If specified, the caller is requesting task-augmented execution for this request.
    /// The request will return a [`CreateTaskResult`] immediately, and the actual result can be
    /// retrieved later via `tasks/result`.
    ///
    /// **Note:** Task augmentation is subject to capability negotiation - receivers **MUST** declare support
    /// for task augmentation of specific request types in their capabilities.
    #[cfg(feature = "tasks")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskMetadata>,
}

/// Controls tool selection behavior for sampling requests.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ToolChoice {
    /// Mode that controls which tools the model can call.
    pub mode: ToolChoiceMode
}

/// Represents the mode that controls which tools the model can call.
/// 
/// - `auto` - Model decides whether to call tools (default).
/// - `required` - Model must call at least one tool.
/// - `none` - Model must not call any tools.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolChoiceMode {
    /// The mode value `auto`.
    #[default]
    Auto,

    /// The mode value `required`.
    Required,

    /// The mode value `none`.
    None
}

/// Specifies the context inclusion options for a request in the Model Context Protocol (MCP).
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMessageResult {
    /// Role of the user who generated the message.
    pub role: Role,
    
    /// Content of the message.
    pub content: OneOrMany<Content>,
    
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
    /// * `endTurn` - The model naturally completed its response.
    /// * `maxTokens` - The response was truncated due to reaching token limits.
    /// * `stopSequence` - A specific stop sequence was encountered during generation.
    /// * `toolUse` - The model wants to use one or more tools.
    /// 
    /// This field is an open string to allow for provider-specific stop reasons.
    #[serde(rename = "stopReason", skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
}

/// Represents reasons why message generation (sampling) stopped, if known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// The model naturally completed its response.
    EndTurn,
    
    /// The response was truncated due to reaching token limits.
    MaxTokens,
    
    /// A specific stop sequence was encountered during generation.
    StopSequence,
    
    /// The model wants to use one or more tools.
    ToolUse,
    
    /// Other stop reasons.
    Other(String)
}

impl Serialize for StopReason {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            StopReason::EndTurn => serializer.serialize_str("endTurn"),
            StopReason::MaxTokens => serializer.serialize_str("maxTokens"),
            StopReason::StopSequence => serializer.serialize_str("stopSequence"),
            StopReason::ToolUse => serializer.serialize_str("toolUse"),
            StopReason::Other(s) => serializer.serialize_str(s),
        }
    }
}

impl<'de> Deserialize<'de> for StopReason {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(StopReason::from(s))
    }
}

impl From<String> for StopReason {
    #[inline]
    fn from(s: String) -> Self {
        match s.as_str() {
            "endTurn" => StopReason::EndTurn,
            "maxTokens" => StopReason::MaxTokens,
            "stopSequence" => StopReason::StopSequence,
            "toolUse" => StopReason::ToolUse,
            _ => StopReason::Other(s),
        }
    }
}

impl From<&str> for StopReason {
    #[inline]
    fn from(s: &str) -> Self {
        match s {
            "endTurn" => StopReason::EndTurn,
            "maxTokens" => StopReason::MaxTokens,
            "stopSequence" => StopReason::StopSequence,
            "toolUse" => StopReason::ToolUse,
            _ => StopReason::Other(s.to_string()),
        }
    }
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
            stop_sequences: None,
            tool_choice: None,
            tools: None,
            #[cfg(feature = "tasks")]
            task: None,
        }
    }
}

impl IntoResponse for CreateMessageResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        match serde_json::to_value(self) {
            Ok(v) => Response::success(req_id, v),
            Err(err) => Response::error(req_id, err.into())
        }
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

impl From<PromptMessage> for SamplingMessage {
    #[inline]
    fn from(msg: PromptMessage) -> Self {
        Self::new(msg.role)
            .with(msg.content)
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
        Self { 
            content: OneOrMany::new(),
            role
        }
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
        self.content.push(content.into());
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

impl ToolChoice {
    /// Creates a new [`ToolChoice`] with [`ToolChoiceMode::Auto`]
    #[inline]
    pub fn auto() -> Self {
        Self { mode: ToolChoiceMode::Auto }
    }

    /// Creates a new [`ToolChoice`] with [`ToolChoiceMode::None`]
    #[inline]
    pub fn none() -> Self {
        Self { mode: ToolChoiceMode::None }
    }

    /// Creates a new [`ToolChoice`] with [`ToolChoiceMode::Required`]
    #[inline]
    pub fn required() -> Self {
        Self { mode: ToolChoiceMode::Required }
    }
    
    /// Returns `true` if the tool choice mode is [`ToolChoiceMode::Auto`]
    #[inline]
    pub fn is_auto(&self) -> bool {
        self.mode == ToolChoiceMode::Auto
    }

    /// Returns `true` if the tool choice mode is [`ToolChoiceMode::None`]
    #[inline]
    pub fn is_none(&self) -> bool {
        self.mode == ToolChoiceMode::None
    }

    /// Returns `true` if the tool choice mode is [`ToolChoiceMode::Required`]
    #[inline]
    pub fn is_required(&self) -> bool {
        self.mode == ToolChoiceMode::Required
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
    
    /// Sets the system prompt for this [`CreateMessageRequestParams`]
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
    
    /// Sets the list of tools that the model can use during generation
    /// 
    /// Default: `None`
    pub fn with_tools<T: IntoIterator<Item = Tool>>(mut self, tools: T) -> Self {
        self.tools = Some(tools
            .into_iter()
            .collect());
        self.with_tool_choice(ToolChoiceMode::Auto)
    }

    /// Sets the control mode for tool selection behavior for sampling requests.
    /// 
    /// Default: `None`
    pub fn with_tool_choice(mut self, mode: ToolChoiceMode) -> Self {
        self.tool_choice = Some(ToolChoice { mode });
        self
    }

    /// Returns an iterator of text messages
    pub fn text(&self) -> impl Iterator<Item = &TextContent> {
        self.msg_iter("text")
            .filter_map(|c| c.as_text())
    }

    /// Returns an iterator of audio messages
    pub fn audio(&self) -> impl Iterator<Item = &AudioContent> {
        self.msg_iter("audio")
            .filter_map(|c| c.as_audio())
    }

    /// Returns an iterator of image messages
    pub fn images(&self) -> impl Iterator<Item = &ImageContent> {
        self.msg_iter("image")
            .filter_map(|c| c.as_image())
    }

    /// Returns an iterator of resource link messages
    pub fn links(&self) -> impl Iterator<Item = &ResourceLink> {
        self.msg_iter("resource_link")
            .filter_map(|c| c.as_link())
    }

    /// Returns an iterator of embedded resource messages
    pub fn resources(&self) -> impl Iterator<Item = &EmbeddedResource> {
        self.msg_iter("resource")
            .filter_map(|c| c.as_resource())
    }

    /// Returns an iterator of tool use messages
    pub fn tools(&self) -> impl Iterator<Item = &ToolUse> {
        self.msg_iter("tool_use")
            .filter_map(|c| c.as_tool())
    }

    /// Returns an iterator of tool execution result messages
    pub fn results(&self) -> impl Iterator<Item = &ToolResult> {
        self.msg_iter("tool_result")
            .filter_map(|c| c.as_result())
    }
    
    /// Returns a messages iterator of a given type
    #[inline]
    fn msg_iter(&self, t: &'static str) -> impl Iterator<Item = &Content> {
        self.messages
            .iter()
            .flat_map(|m| m.content.as_slice())
            .filter(move |c| c.get_type() == t)
    }
}

impl CreateMessageResult {
    /// Creates a new [`CreateMessageResult`]
    #[inline]
    pub fn new(role: Role) -> Self {
        Self {
            stop_reason: None,
            model: String::new(),
            content: OneOrMany::new(),
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
    pub fn with_stop_reason(mut self, reason: impl Into<StopReason>) -> Self {
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
        self.content.push(content.into());
        self
    }
    
    /// Marks that model completed the response
    #[inline]
    pub fn end_turn(self) -> Self {
        self.with_stop_reason(StopReason::EndTurn)
    }
    
    /// Requests a tool use and sets the stop reason to `toolUse`
    pub fn use_tool<N, Args>(self, name: N, args: Args) -> Self
    where 
        N: Into<String>,
        Args: IntoArgs
    {
        self.with_content(ToolUse::new(name, args))
            .with_stop_reason(StopReason::ToolUse)
    }

    /// Requests to use a set of tools and sets the stop reason to `toolUse`
    pub fn use_tools<N, Args>(self, tools: impl IntoIterator<Item = (N, Args)>) -> Self
    where
        N: Into<String>,
        Args: IntoArgs
    {
        tools.into_iter()
            .fold(self, |acc, (name, args)| acc.use_tool(name, args))
            .with_stop_reason(StopReason::ToolUse)
    }

    /// Returns an iterator of text messages
    pub fn text(&self) -> impl Iterator<Item = &TextContent> {
        self.msg_iter("text")
            .filter_map(|c| c.as_text())
    }

    /// Returns an iterator of audio content
    pub fn audio(&self) -> impl Iterator<Item = &AudioContent> {
        self.msg_iter("audio")
            .filter_map(|c| c.as_audio())
    }

    /// Returns an iterator of image content
    pub fn images(&self) -> impl Iterator<Item = &ImageContent> {
        self.msg_iter("image")
            .filter_map(|c| c.as_image())
    }

    /// Returns an iterator of resource link content
    pub fn links(&self) -> impl Iterator<Item = &ResourceLink> {
        self.msg_iter("resource_link")
            .filter_map(|c| c.as_link())
    }

    /// Returns an iterator of embedded resource content
    pub fn resources(&self) -> impl Iterator<Item = &EmbeddedResource> {
        self.msg_iter("resource")
            .filter_map(|c| c.as_resource())
    }

    /// Returns an iterator of tool use content
    pub fn tools(&self) -> impl Iterator<Item = &ToolUse> {
        self.msg_iter("tool_use")
            .filter_map(|c| c.as_tool())
    }

    /// Returns an iterator of tool execution result content
    pub fn results(&self) -> impl Iterator<Item = &ToolResult> {
        self.msg_iter("tool_result")
            .filter_map(|c| c.as_result())
    }
    
    /// Returns a content iterator of a given type
    #[inline]
    fn msg_iter(&self, t: &'static str) -> impl Iterator<Item = &Content> {
        self.content
            .iter()
            .filter(move |c| c.get_type() == t)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_sets_auto_tool_choice_mode_by_default() {
        let mode = ToolChoiceMode::default();

        assert_eq!(mode, ToolChoiceMode::Auto);
    }

    #[test]
    fn it_sets_auto_tool_choice_by_default() {
        let tool_choice = ToolChoice::default();

        assert_eq!(tool_choice.mode, ToolChoiceMode::Auto);
    }

    #[test]
    #[cfg(feature = "server")]
    fn it_sets_auto_tool_choice_when_tools_specified() {
        let params = CreateMessageRequestParams::new()
            .with_tools([
                Tool::new("test 1", async || "test 1"),
                Tool::new("test 2", async || "test 2")
            ]);

        assert_eq!(params.tool_choice.unwrap().mode, ToolChoiceMode::Auto);
    }

    #[test]
    fn it_sets_tool_choice() {
        let params = CreateMessageRequestParams::new()
            .with_tool_choice(ToolChoiceMode::Required);

        assert_eq!(params.tool_choice.unwrap().mode, ToolChoiceMode::Required);
    }

    #[test]
    fn it_builds_sampling_message() {
        let msg = SamplingMessage::user()
            .with("Hello");

        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content.len(), 1);
    }

    #[test]
    fn it_builds_create_message_request_params() {
        let params = CreateMessageRequestParams::new()
            .with_message("Hello")
            .with_sys_prompt("System prompt")
            .with_max_tokens(100)
            .with_temp(0.7);

        assert_eq!(params.messages.len(), 1);
        assert_eq!(params.sys_prompt.as_deref(), Some("System prompt"));
        assert_eq!(params.max_tokens, 100);
        assert_eq!(params.temp, Some(0.7));
    }

    #[test]
    fn it_sets_context_inclusion() {
        let params = CreateMessageRequestParams::new()
            .with_no_ctx();
        assert!(matches!(params.include_context, Some(ContextInclusion::None)));

        let params = CreateMessageRequestParams::new()
            .with_this_server();
        assert!(matches!(params.include_context, Some(ContextInclusion::ThisServer)));

        let params = CreateMessageRequestParams::new()
            .with_all_servers();
        assert!(matches!(params.include_context, Some(ContextInclusion::AllServers)));
    }

    #[test]
    fn it_builds_create_message_result() {
        let result = CreateMessageResult::assistant()
            .with_model("gpt-4")
            .with_content("Hello world")
            .end_turn();

        assert_eq!(result.role, Role::Assistant);
        assert_eq!(result.model, "gpt-4");
        assert_eq!(result.content.len(), 1);
        assert_eq!(result.stop_reason, Some(StopReason::EndTurn));
    }

    #[test]
    fn it_handles_tool_use_in_result() {
        let result = CreateMessageResult::assistant()
            .use_tool("calculator", ());

        assert_eq!(result.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(result.content.len(), 1);

        let tool_use = result.tools().next().unwrap();
        assert_eq!(tool_use.name, "calculator");
    }

    #[test]
    fn it_adds_model_hints() {
        let pref = ModelPreferences::new()
            .with_hint("claude")
            .with_hints(["gpt-4", "llama"]);

        assert_eq!(pref.hints.as_ref().unwrap().len(), 3);
        assert_eq!(pref.hints.as_ref().unwrap()[0].name.as_deref(), Some("claude"));
    }
    
    #[test]
    fn it_converts_stop_reason_from_str() {
        let reasons = [
            (StopReason::ToolUse, "toolUse"),
            (StopReason::MaxTokens, "maxTokens"),
            (StopReason::EndTurn, "endTurn"),
            (StopReason::StopSequence, "stopSequence"),
            (StopReason::Other("test".to_string()), "test")
        ];

        for (expected, reason_str) in reasons {
            let reason = StopReason::from(reason_str);
            assert_eq!(reason, expected);
        }
    }

    #[test]
    fn it_converts_stop_reason_from_string() {
        let reasons = [
            (StopReason::ToolUse, "toolUse"),
            (StopReason::MaxTokens, "maxTokens"),
            (StopReason::EndTurn, "endTurn"),
            (StopReason::StopSequence, "stopSequence"),
            (StopReason::Other("test".to_string()), "test")
        ];

        for (expected, reason_str) in reasons {
            let reason = StopReason::from(reason_str.to_string());
            assert_eq!(reason, expected);
        }
    }
    
    #[test]
    fn it_serializes_stop_reason() {
        let reasons = [
            (StopReason::ToolUse, "\"toolUse\""),
            (StopReason::MaxTokens, "\"maxTokens\""),
            (StopReason::EndTurn, "\"endTurn\""),
            (StopReason::StopSequence, "\"stopSequence\""),
            (StopReason::Other("test".to_string()), "\"test\"")
        ];

        for (reason, expected) in reasons {
            let json = serde_json::to_string(&reason).unwrap();
            assert_eq!(json, expected);
        }
    }

    #[test]
    fn it_deserializes_stop_reason() {
        let reasons = [
            (StopReason::ToolUse, "\"toolUse\""),
            (StopReason::MaxTokens, "\"maxTokens\""),
            (StopReason::EndTurn, "\"endTurn\""),
            (StopReason::StopSequence, "\"stopSequence\""),
            (StopReason::Other("test".to_string()), "\"test\"")
        ];

        for (expected, reason_str) in reasons {
            let reason: StopReason = serde_json::from_str(reason_str).unwrap();
            assert_eq!(reason, expected);
        }
    }
    
    #[test]
    fn it_serializes_model_preferences() {
        let pref = ModelPreferences::new()
            .with_cost_priority(0.5)
            .with_speed_priority(0.75)
            .with_intel_priority(0.25);
        
        let json = serde_json::to_string(&pref).unwrap();
        
        let expected = r#"{"costPriority":0.5,"speedPriority":0.75,"intelligencePriority":0.25}"#;
        assert_eq!(json, expected);
    }
    
    #[test]
    fn it_deserializes_model_preferences() {
        let json = r#"{"costPriority":0.5,"speedPriority":0.75,"intelligencePriority":0.25}"#;
        let pref: ModelPreferences = serde_json::from_str(json).unwrap();
        
        assert_eq!(pref.cost_priority, Some(0.5));
        assert_eq!(pref.speed_priority, Some(0.75));
        assert_eq!(pref.intelligence_priority, Some(0.25));
    }
}
//! Types that describes server and client capabilities

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Represents the capabilities that a client may support.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCapabilities {
    /// Gets or sets the client's roots capability, which are entry points for resource navigation.
    ///
    /// > **Note:** When `roots` is `Some`, the client indicates that it can respond to
    /// > server requests for listing root URIs. Root URIs serve as entry points for resource navigation in the protocol.
    /// >
    /// > The server can use `RequestRoots` to request the list of
    /// > available roots from the client, which will trigger the client's `RootsHandler`.
    #[cfg(any(feature = "legacy-spec", feature = "client"))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roots: Option<RootsCapability>,

    /// Gets or sets the client's sampling capability, which indicates whether the client
    /// supports issuing requests to an LLM on behalf of the server.
    #[cfg(any(feature = "legacy-spec", feature = "client"))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<SamplingCapability>,

    /// Gets or sets the client's elicitation capability, which indicates whether the client
    /// supports elicitation of additional information from the user on behalf of the server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elicitation: Option<ElicitationCapability>,

    /// Present if the client supports task-augmented requests.
    ///
    /// Under MCP 2026-07-28, tasks become an extension; this top-level
    /// field is replaced by an entry in the `extensions` map keyed by
    /// `io.modelcontextprotocol/tasks`.
    #[cfg(all(feature = "tasks", any(feature = "legacy-spec", feature = "client")))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tasks: Option<ClientTasksCapability>,

    /// Protocol extensions the client supports.
    ///
    /// Keyed by reverse-DNS extension id (e.g. `io.modelcontextprotocol/tasks`)
    /// mapping to that extension's capability value. Replaces the former
    /// top-level `tasks` capability under MCP 2026-07-28.
    ///
    /// Unconditional, unlike its counterpart on
    /// [`ServerCapabilities`]: a legacy `initialize` handshake has nowhere to
    /// put a *server's* capability map, but it carries the client's fine, and
    /// that is the channel MCP Apps negotiates over against every host shipping
    /// today. Older peers ignore what they do not know.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<HashMap<String, serde_json::Value>>,

    /// Gets or sets experimental, non-standard capabilities that the client supports.
    ///
    /// > **Note:** The `experimental` map allows clients to advertise support for features that are not yet
    /// > standardized in the Model Context Protocol specification. This extension mechanism enables
    /// > future protocol enhancements while maintaining backward compatibility.
    /// >
    /// > Values in this map are implementation-specific and should be coordinated between client
    /// > and server implementations. Servers should not assume the presence of any experimental capability
    /// > without checking for it first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental: Option<HashMap<String, serde_json::Value>>,
}

/// The capabilities a client declares on a single request (MCP 2026-07-28).
///
/// The `io.modelcontextprotocol/clientCapabilities` value of a request's
/// `_meta`. There is no handshake under 2026-07-28, so this is where a server
/// learns what the caller of *this* request can do: which input requests it can
/// answer, and which extensions it supports.
///
/// A wrapper rather than more fields on
/// [`ClientMrtrCapabilities`](crate::types::mrtr::ClientMrtrCapabilities), so
/// that type stays `Copy`. The MRTR flags sit flat beside `extensions` on the
/// wire, exactly as the spec's `ClientCapabilities` spells them.
///
/// # Examples
/// ```
/// use neva::types::RequestClientCapabilities;
///
/// let caps: RequestClientCapabilities = serde_json::from_value(serde_json::json!({
///     "elicitation": {},
///     "extensions": {
///         "io.modelcontextprotocol/ui": { "mimeTypes": ["text/html;profile=mcp-app"] }
///     }
/// }))?;
///
/// assert!(caps.mrtr.elicitation.is_some());
/// assert!(caps.extension("io.modelcontextprotocol/ui").is_some());
/// assert!(caps.extension("com.example/other").is_none());
/// # Ok::<(), serde_json::Error>(())
/// ```
#[cfg(not(feature = "legacy-spec"))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RequestClientCapabilities {
    /// The input-request kinds the client can answer.
    #[serde(flatten)]
    pub mrtr: crate::types::mrtr::ClientMrtrCapabilities,

    /// Protocol extensions the client supports, keyed by reverse-DNS extension
    /// id and mapping to that extension's settings.
    ///
    /// The same map as [`ClientCapabilities::extensions`], declared per request
    /// instead of on `initialize`.
    ///
    /// A value that is not an object reads as no extensions declared.
    #[serde(
        default,
        deserialize_with = "de_extensions",
        skip_serializing_if = "Option::is_none"
    )]
    pub extensions: Option<HashMap<String, serde_json::Value>>,
}

/// Reads `extensions`, tolerating a malformed value as "none declared".
///
/// A request's `_meta` is parsed as one unit, so a hard error here would take
/// the progress token, log level and every other key down with it -- for a map
/// neva ignored outright before it read extensions at all.
#[cfg(not(feature = "legacy-spec"))]
fn de_extensions<'de, D>(
    deserializer: D,
) -> Result<Option<HashMap<String, serde_json::Value>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(
        match Option::<serde_json::Value>::deserialize(deserializer)? {
            Some(serde_json::Value::Object(map)) => Some(map.into_iter().collect()),
            _ => None,
        },
    )
}

#[cfg(not(feature = "legacy-spec"))]
impl RequestClientCapabilities {
    /// The settings the client declared for extension `id`, or `None` when it
    /// did not declare that extension.
    ///
    /// Presence is the declaration, but what counts as *supporting* an
    /// extension is up to that extension: MCP Apps, for one, requires its
    /// settings to name the content types the client renders.
    ///
    /// # Examples
    /// ```
    /// use neva::types::RequestClientCapabilities;
    ///
    /// let caps: RequestClientCapabilities = serde_json::from_value(serde_json::json!({
    ///     "extensions": { "com.example/search": { "fuzzy": true } }
    /// }))?;
    /// 
    /// assert_eq!(
    ///     caps.extension("com.example/search"),
    ///     Some(&serde_json::json!({ "fuzzy": true }))
    /// );
    /// # Ok::<(), serde_json::Error>(())
    /// ```
    #[inline]
    pub fn extension(&self, id: &str) -> Option<&serde_json::Value> {
        self.extensions.as_ref()?.get(id)
    }
}

/// Represents a client capability that enables root resource discovery in the Model Context Protocol.
///
/// > **Note:** When present in [`ClientCapabilities`], it indicates that the client supports listing
/// > root URIs that serve as entry points for resource navigation.
/// >
/// > The roots capability establishes a mechanism for servers to discover and access the hierarchical
/// > structure of resources provided by a client. Root URIs represent top-level entry points from which
/// > servers can navigate to access specific resources.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[cfg(any(feature = "legacy-spec", feature = "client"))]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct RootsCapability {
    /// Indicates whether the client supports notifications for changes to the roots list.
    ///
    /// > **Note:** When set to `true`, the client can notify servers when roots are added,
    /// > removed, or modified, allowing servers to refresh their roots cache accordingly.
    /// > This enables servers to stay synchronized with client-side changes to available roots.
    #[serde(default, rename = "listChanged")]
    pub list_changed: bool,
}

/// Represents the capability for a client to generate text or other content using an AI model.
///
/// > **Note:** This capability enables the MCP client to respond to sampling requests from an MCP server.
/// >
/// > When this capability is enabled, an MCP server can request the client to generate content
/// > using an AI model. The client must set a `SamplingHandler` to process these requests.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[cfg(any(feature = "legacy-spec", feature = "client"))]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct SamplingCapability {
    /// Indicates whether the client supports context inclusion via `includeContext` parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<SamplingContextCapability>,

    /// Indicates whether the client supports tool use via `tools` and `toolChoice` parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<SamplingToolsCapability>,
}

/// Represents the sampling context capability.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[cfg(any(feature = "legacy-spec", feature = "client"))]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct SamplingContextCapability {
    // Currently empty in the spec, but may be extended in the future
}

/// Represents the sampling tools capability.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[cfg(any(feature = "legacy-spec", feature = "client"))]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct SamplingToolsCapability {
    // Currently empty in the spec, but may be extended in the future
}

/// Represents the capability for a client to provide server-requested additional information during interactions.
///
/// > **Note:** This capability enables the MCP client to respond to elicitation requests from an MCP server.
/// >
/// > When this capability is enabled, an MCP server can request the client to provide additional information
/// > during interactions.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationCapability {
    /// Indicates whether the client supports `form` mode elicitation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub form: Option<ElicitationFormCapability>,

    /// Indicates whether the client supports `url` mode elicitation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<ElicitationUrlCapability>,
}

/// Represents elicitation form capability.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationFormCapability {
    // Currently empty in the spec, but may be extended in the future.
}

/// Represents elicitation URL capability
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationUrlCapability {
    // Currently empty in the spec, but may be extended in the future.
}

/// Represents the capabilities that a server may support.
///
/// > **Note:** Server capabilities define the features and functionality available when clients connect.
/// > These capabilities are advertised to clients during the initialize handshake.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilities {
    /// Present if the server offers any tools to call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsCapability>,

    /// Present if the server offers any prompt templates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<PromptsCapability>,

    /// Present if the server offers any resources to read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourcesCapability>,

    /// Present if the server supports sending log messages to the client.
    #[cfg(feature = "legacy-spec")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<LoggingCapability>,

    /// Present if the server supports argument autocompletion suggestions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completions: Option<CompletionsCapability>,

    /// Present if the server supports task-augmented requests.
    ///
    /// Under MCP 2026-07-28, tasks become an extension; this top-level
    /// field is replaced by an entry in the `extensions` map keyed by
    /// `io.modelcontextprotocol/tasks`.
    ///
    /// Compiled for the 2026-07-28 **client** too: after a dual-mode fallback a
    /// legacy peer advertises tasks here, and the field must survive
    /// deserialization of its `initialize` result. A 2026-07-28 server never
    /// sets it, and `skip_serializing_if` keeps the 2026-07-28 wire unchanged.
    #[cfg(all(feature = "tasks", any(feature = "legacy-spec", feature = "client")))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tasks: Option<ServerTasksCapability>,

    /// Protocol extensions the server supports (MCP 2026-07-28).
    ///
    /// Keyed by reverse-DNS extension id (e.g. `io.modelcontextprotocol/tasks`)
    /// mapping to that extension's capability value. Replaces the former
    /// top-level `tasks` capability under MCP 2026-07-28.
    #[cfg(not(feature = "legacy-spec"))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<HashMap<String, serde_json::Value>>,

    /// Indicates experimental, non-standard capabilities that the server supports.
    ///
    /// > **Note:** The `experimental` map allows servers to advertise support for features that are not yet
    /// > standardized in the Model Context Protocol specification. This extension mechanism enables
    /// > future protocol enhancements while maintaining backward compatibility.
    /// >
    /// > Values in this dictionary are implementation-specific and should be coordinated between client
    /// > and server implementations. Clients should not assume the presence of any experimental capability
    /// > without checking for it first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental: Option<HashMap<String, serde_json::Value>>,
}

/// Represents the tools capability configuration.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct ToolsCapability {
    /// Indicates whether this server supports notifications for changes to the tool list.
    #[serde(default, rename = "listChanged")]
    pub list_changed: bool,
}

/// Represents the prompts capability configuration.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct PromptsCapability {
    /// Indicates whether this server supports notifications for changes to the prompt list.
    #[serde(default, rename = "listChanged")]
    pub list_changed: bool,
}

/// Represents the resources capability configuration.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct ResourcesCapability {
    /// Indicates whether this server supports notifications for changes to the resource list.
    #[serde(default, rename = "listChanged")]
    pub list_changed: bool,

    /// Indicates whether this server supports subscribing to resource updates.
    #[serde(default)]
    pub subscribe: bool,
}

/// Represents the logging capability configuration.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[cfg(feature = "legacy-spec")]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct LoggingCapability {
    // Currently empty in the spec, but may be extended in the future
}

/// Represents the completions capability configuration.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct CompletionsCapability {
    // Currently empty in the spec, but may be extended in the future
}

/// Represents task-augmented requests capability configuration for a server.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[cfg(feature = "tasks")]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct ServerTasksCapability {
    /// Indicates whether this server supports `tasks/cancel`.
    #[cfg(feature = "legacy-spec")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel: Option<TaskCancellationCapability>,

    /// Indicates whether this server supports `tasks/list`.
    #[cfg(feature = "legacy-spec")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list: Option<TaskListCapability>,

    /// Specifies which request types can be augmented with tasks.
    #[cfg(feature = "legacy-spec")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requests: Option<ServerTaskRequestsCapability>,
}

/// Represents task-augmented requests capability configuration for a client.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[cfg(feature = "tasks")]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct ClientTasksCapability {
    /// Indicates whether this client supports `tasks/cancel`.
    #[cfg(feature = "legacy-spec")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel: Option<TaskCancellationCapability>,

    /// Indicates whether this client supports `tasks/list`.
    #[cfg(feature = "legacy-spec")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list: Option<TaskListCapability>,

    /// Specifies which request types can be augmented with tasks.
    #[cfg(feature = "legacy-spec")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requests: Option<ClientTaskRequestsCapability>,
}

/// Represents task cancellation capability configuration.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[cfg(all(feature = "tasks", feature = "legacy-spec"))]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct TaskCancellationCapability {
    // Currently empty in the spec, but may be extended in the future
}

/// Represents task list retrieval capability configuration.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[cfg(all(feature = "tasks", feature = "legacy-spec"))]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct TaskListCapability {
    // Currently empty in the spec, but may be extended in the future
}

/// Specifies which request types can be augmented with tasks.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[cfg(all(feature = "tasks", feature = "legacy-spec"))]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct ServerTaskRequestsCapability {
    /// Specifies task support for tool-related requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsTaskCapability>,
}

/// Specifies which request types can be augmented with tasks.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[cfg(all(feature = "tasks", feature = "legacy-spec"))]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct ClientTaskRequestsCapability {
    /// Specifies task support for elicitation-related requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elicitation: Option<ElicitationTaskCapability>,

    /// Specifies task support for sampling-related requests.
    #[cfg(feature = "legacy-spec")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<SamplingTaskCapability>,
}

/// Specifies task support for tool-related requests.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[cfg(all(feature = "tasks", feature = "legacy-spec"))]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct ToolsTaskCapability {
    /// Indicates whether the server supports task-augmented `tools/call` requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call: Option<ToolsCallTaskCapability>,
}

/// Specifies task support for elicitation-related requests.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[cfg(all(feature = "tasks", feature = "legacy-spec"))]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationTaskCapability {
    /// Indicates whether the client supports task-augmented `elicitation/create` requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create: Option<ElicitationCreateTaskCapability>,
}

/// Specifies task support for sampling-related requests.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[cfg(all(feature = "tasks", feature = "legacy-spec"))]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct SamplingTaskCapability {
    /// Indicates whether the client supports task-augmented `sampling/createMessage` requests.
    #[serde(rename = "createMessage", skip_serializing_if = "Option::is_none")]
    pub create: Option<SamplingCreateMessageTaskCapability>,
}

/// Represents task support configuration for `tools/call` requests.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[cfg(all(feature = "tasks", feature = "legacy-spec"))]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct ToolsCallTaskCapability {
    // Currently empty in the spec, but may be extended in the future
}

/// Represents task support configuration for `elicitation/create` requests.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[cfg(all(feature = "tasks", feature = "legacy-spec"))]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationCreateTaskCapability {
    // Currently empty in the spec, but may be extended in the future
}

/// Represents task support configuration for `sampling/createMessage` requests.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[cfg(all(feature = "tasks", feature = "legacy-spec"))]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct SamplingCreateMessageTaskCapability {
    // Currently empty in the spec, but may be extended in the future
}

#[cfg(feature = "server")]
impl ToolsCapability {
    /// Specifies whether this server supports notifications for changes to the tools list.
    ///
    /// Default: _false_
    pub fn with_list_changed(mut self) -> Self {
        self.list_changed = true;
        self
    }
}

#[cfg(feature = "server")]
impl ResourcesCapability {
    /// Specifies whether this server supports notifications for changes to the resource list.
    ///
    /// Default: _false_
    pub fn with_list_changed(mut self) -> Self {
        self.list_changed = true;
        self
    }

    /// Specifies whether this server supports subscribing to resource updates.
    ///
    /// Default: _false_
    pub fn with_subscribe(mut self) -> Self {
        self.subscribe = true;
        self
    }
}

#[cfg(feature = "server")]
impl PromptsCapability {
    /// Specifies whether this server supports notifications for changes to the prompts list.
    ///
    /// Default: _false_
    pub fn with_list_changed(mut self) -> Self {
        self.list_changed = true;
        self
    }
}

#[cfg(feature = "client")]
impl RootsCapability {
    /// Specifies whether this client supports notifications for changes to the roots list.
    ///
    /// Default: _false_
    pub fn with_list_changed(mut self) -> Self {
        self.list_changed = true;
        self
    }
}

#[cfg(feature = "client")]
impl SamplingCapability {
    /// Specifies whether this client supports context inclusion.
    ///
    /// Default: `None`
    pub fn with_context(mut self) -> Self {
        self.context = Some(SamplingContextCapability {});
        self
    }

    /// Specifies whether this client supports the tool use feature.
    ///
    /// Default: `None`
    pub fn with_tools(mut self) -> Self {
        self.tools = Some(SamplingToolsCapability {});
        self
    }
}

#[cfg(feature = "client")]
impl ElicitationCapability {
    /// Specifies whether this client supports `form` elicitation mode.
    ///
    /// Default: `None`
    pub fn with_form(mut self) -> Self {
        self.form = Some(ElicitationFormCapability {});
        self
    }

    /// Specifies whether this client supports `url` elicitation mode.
    ///
    /// Default: `None`
    pub fn with_url(mut self) -> Self {
        self.url = Some(ElicitationUrlCapability {});
        self
    }
}

#[cfg(all(feature = "server", feature = "tasks", feature = "legacy-spec"))]
impl ServerTasksCapability {
    /// Specifies whether this server supports `tasks/cancel` requests
    pub fn with_cancel(mut self) -> Self {
        self.cancel = Some(TaskCancellationCapability {});
        self
    }

    /// Specifies whether this server supports `tasks/list` requests
    pub fn with_list(mut self) -> Self {
        self.list = Some(TaskListCapability {});
        self
    }

    /// Specifies whether this server supports task-augmented requests
    pub fn with_requests<F>(mut self, config: F) -> Self
    where
        F: FnOnce(ServerTaskRequestsCapability) -> ServerTaskRequestsCapability,
    {
        self.requests = Some(config(Default::default()));
        self
    }

    /// Specifies whether this server supports task-augmented tools-related requests
    pub fn with_tools(self) -> Self {
        self.with_requests(|req| req.with_tools())
    }

    /// Specifies whether this server supports all task-augmented capabilities
    pub fn with_all(self) -> Self {
        self.with_cancel().with_list().with_tools()
    }
}

#[cfg(all(feature = "client", feature = "tasks", feature = "legacy-spec"))]
impl ClientTasksCapability {
    /// Specifies whether this client supports `tasks/cancel` requests
    pub fn with_cancel(mut self) -> Self {
        self.cancel = Some(TaskCancellationCapability {});
        self
    }

    /// Specifies whether this client supports `tasks/list` requests
    pub fn with_list(mut self) -> Self {
        self.list = Some(TaskListCapability {});
        self
    }

    /// Specifies whether this client supports task-augmented requests
    pub fn with_requests<F>(mut self, config: F) -> Self
    where
        F: FnOnce(ClientTaskRequestsCapability) -> ClientTaskRequestsCapability,
    {
        self.requests = Some(config(Default::default()));
        self
    }

    /// Specifies whether this client supports task-augmented elicitation-related requests
    pub fn with_elicitation(self) -> Self {
        self.with_requests(|req| req.with_elicitation())
    }

    /// Specifies whether this client supports task-augmented sampling-related requests
    #[cfg(feature = "legacy-spec")]
    pub fn with_sampling(self) -> Self {
        self.with_requests(|req| req.with_sampling())
    }

    /// Specifies whether this client supports all task-augmented capabilities
    pub fn with_all(self) -> Self {
        #[cfg(feature = "legacy-spec")]
        return self
            .with_cancel()
            .with_list()
            .with_elicitation()
            .with_sampling();
        #[cfg(not(feature = "legacy-spec"))]
        return self.with_cancel().with_list().with_elicitation();
    }
}

#[cfg(all(feature = "server", feature = "tasks", feature = "legacy-spec"))]
impl ServerTaskRequestsCapability {
    /// Specifies task support for tool-related requests.
    pub fn with_tools(mut self) -> Self {
        self.tools = Some(ToolsTaskCapability {
            call: Some(ToolsCallTaskCapability {}),
        });
        self
    }
}

#[cfg(all(feature = "client", feature = "tasks", feature = "legacy-spec"))]
impl ClientTaskRequestsCapability {
    /// Specifies task support for elicitation-related requests.
    pub fn with_elicitation(mut self) -> Self {
        self.elicitation = Some(ElicitationTaskCapability {
            create: Some(ElicitationCreateTaskCapability {}),
        });
        self
    }

    /// Specifies task support for sampling-related requests.
    #[cfg(feature = "legacy-spec")]
    pub fn with_sampling(mut self) -> Self {
        self.sampling = Some(SamplingTaskCapability {
            create: Some(SamplingCreateMessageTaskCapability {}),
        });
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resources_capability_states_only_what_it_supports() {
        // Every member of a capability object is optional: a server that
        // supports neither subscriptions nor list-change notifications
        // advertises `"resources": {}`, and one that supports only the latter
        // says so alone. Requiring a member turns a conformant peer's
        // capabilities into a parse error and drops the connection.
        let empty: ResourcesCapability = serde_json::from_str("{}").unwrap();
        assert!(!empty.subscribe);
        assert!(!empty.list_changed);

        let listing: ResourcesCapability =
            serde_json::from_str(r#"{"listChanged": true}"#).unwrap();
        assert!(!listing.subscribe);
        assert!(listing.list_changed);

        let subscribing: ResourcesCapability =
            serde_json::from_str(r#"{"subscribe": true}"#).unwrap();
        assert!(subscribing.subscribe);
        assert!(!subscribing.list_changed);
    }

    /// The per-request `clientCapabilities` of MCP 2026-07-28: the MRTR flags
    /// and the `extensions` map, side by side in one object.
    #[cfg(not(feature = "legacy-spec"))]
    mod request_client_capabilities {
        use super::*;
        use serde_json::json;

        #[test]
        fn the_mrtr_flags_and_the_extensions_map_are_read_from_one_object() {
            let caps: RequestClientCapabilities = serde_json::from_value(json!({
                "elicitation": { "form": {} },
                "roots": {},
                "extensions": {
                    "io.modelcontextprotocol/ui": { "mimeTypes": ["text/html;profile=mcp-app"] }
                }
            }))
            .unwrap();

            let modes = caps.mrtr.elicitation.expect("elicitation declared");
            assert!(modes.form && !modes.url);
            assert!(caps.mrtr.roots && !caps.mrtr.sampling);
            assert_eq!(
                caps.extension("io.modelcontextprotocol/ui"),
                Some(&json!({ "mimeTypes": ["text/html;profile=mcp-app"] }))
            );
        }

        #[test]
        fn the_legacy_boolean_flags_still_parse_through_the_wrapper() {
            // The flattened struct is decoded from a buffer rather than the
            // JSON itself; the boolean tolerance must survive that.
            let caps: RequestClientCapabilities =
                serde_json::from_value(json!({ "elicitation": true, "sampling": false })).unwrap();

            assert!(caps.mrtr.elicitation.is_some_and(|m| m.unconstrained()));
            assert!(!caps.mrtr.sampling);
            assert!(caps.extensions.is_none());
        }

        #[test]
        fn it_is_written_flat_and_omits_an_absent_map() {
            assert_eq!(
                serde_json::to_value(RequestClientCapabilities::default()).unwrap(),
                json!({})
            );

            let caps = RequestClientCapabilities {
                mrtr: crate::types::mrtr::ClientMrtrCapabilities {
                    elicitation: Some(Default::default()),
                    ..Default::default()
                },
                extensions: Some(HashMap::from([(
                    "com.example/search".to_string(),
                    json!({}),
                )])),
            };
            let json = serde_json::to_value(&caps).unwrap();

            assert_eq!(
                json,
                json!({ "elicitation": {}, "extensions": { "com.example/search": {} } })
            );

            let back: RequestClientCapabilities = serde_json::from_value(json).unwrap();
            assert!(back.mrtr.elicitation.is_some());
            assert!(back.extension("com.example/search").is_some());
        }

        #[test]
        fn a_malformed_map_declares_nothing_and_takes_nothing_else_down() {
            for extensions in [json!([]), json!("io.modelcontextprotocol/ui"), json!(null)] {
                let caps: RequestClientCapabilities = serde_json::from_value(json!({
                    "elicitation": {},
                    "extensions": extensions
                }))
                .unwrap_or_else(|err| panic!("{extensions} must not fail the parse: {err}"));

                assert!(caps.extensions.is_none(), "{extensions}");
                assert!(caps.mrtr.elicitation.is_some(), "{extensions}");
            }
        }
    }
}

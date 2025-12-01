//! Types that describes server and client capabilities

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roots: Option<RootsCapability>,

    /// Gets or sets the client's sampling capability, which indicates whether the client 
    /// supports issuing requests to an LLM on behalf of the server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<SamplingCapability>,

    /// Gets or sets the client's elicitation capability, which indicates whether the client 
    /// supports elicitation of additional information from the user on behalf of the server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elicitation: Option<ElicitationCapability>,
    
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
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct RootsCapability {
    /// Indicates whether the client supports notifications for changes to the roots list.
    ///
    /// > **Note:** When set to `true`, the client can notify servers when roots are added, 
    /// > removed, or modified, allowing servers to refresh their roots cache accordingly.
    /// > This enables servers to stay synchronized with client-side changes to available roots.
    #[serde(default, rename = "listChanged")]
    pub list_changed: bool
}

/// Represents the capability for a client to generate text or other content using an AI model.
///
/// > **Note:** This capability enables the MCP client to respond to sampling requests from an MCP server.
/// >
/// > When this capability is enabled, an MCP server can request the client to generate content
/// > using an AI model. The client must set a `SamplingHandler` to process these requests.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct SamplingCapability {
    /// Indicates whether the client supports context inclusion via `includeContext` parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<SamplingContextCapability>,

    /// Indicates whether the client supports tool use via `tools` and `toolChoice` parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<SamplingToolsCapability>
}

/// Represents the sampling context capability.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct SamplingContextCapability {
    // Currently empty in the spec, but may be extended in the future
}

/// Represents the sampling tools capability.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct SamplingToolsCapability {
    // Currently empty in the spec, but may be extended in the future
}

/// Represents the capability for a client to provide server-requested additional information during interactions.
/// 
/// > **Note:** This capability enables the MCP client to respond to elicitation requests from an MCP server.
/// >
/// > When this capability is enabled, an MCP server can request the client to provide additional information
/// > during interactions. The client must set a <see cref="ElicitationHandler"/> to process these requests.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationCapability {
    /// Indicates whether the client supports `form` mode elicitation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub form: Option<ElicitationFormCapability>,

    /// Indicates whether the client supports `url` mode elicitation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<ElicitationUrlCapability>
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<LoggingCapability>,
    
    /// Present if the server supports argument autocompletion suggestions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completions: Option<CompletionsCapability>,

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
    pub list_changed: bool
}

/// Represents the prompts capability configuration.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct PromptsCapability {
    /// Indicates whether this server supports notifications for changes to the prompt list.
    #[serde(default, rename = "listChanged")]
    pub list_changed: bool
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
    pub subscribe: bool
}

/// Represents the logging capability configuration.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
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

impl ToolsCapability {
    /// Specifies whether this server supports notifications for changes to the tools list.
    ///
    /// Default: _false_
    pub fn with_list_changed(mut self) -> Self {
        self.list_changed = true;
        self
    }
}

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

impl PromptsCapability {
    /// Specifies whether this server supports notifications for changes to the prompts list.
    ///
    /// Default: _false_
    pub fn with_list_changed(mut self) -> Self {
        self.list_changed = true;
        self
    }
}

impl RootsCapability {
    /// Specifies whether this client supports notifications for changes to the roots list.
    ///
    /// Default: _false_
    pub fn with_list_changed(mut self) -> Self {
        self.list_changed = true;
        self
    }
}

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
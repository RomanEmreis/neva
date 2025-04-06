//! Types that describes server and client capabilities

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// Represents the capabilities that a client may support.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Debug, Clone, Deserialize)]
pub struct ClientCapabilities {
    pub experimental: Option<HashMap<String, serde_json::Value>>,
}

/// Represents the capabilities that a server may support.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Debug, Clone, Serialize)]
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
}

/// Represents the tools capability configuration.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Default, Debug, Clone, Serialize)]
pub struct ToolsCapability {
    /// Gets or sets whether this server supports notifications for changes to the tool list.
    #[serde(rename = "listChanged")]
    pub list_changed: bool
}

/// Represents the prompts capability configuration.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Default, Debug, Clone, Serialize)]
pub struct PromptsCapability {
    /// Whether this server supports notifications for changes to the prompt list.
    #[serde(rename = "listChanged")]
    pub list_changed: bool
}

/// Represents the resources capability configuration.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Default, Debug, Clone, Serialize)]
pub struct ResourcesCapability {
    /// Whether this server supports notifications for changes to the resource list.
    #[serde(rename = "listChanged")]
    pub list_changed: bool,

    /// Whether this server supports subscribing to resource updates.
    pub subscribe: bool
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
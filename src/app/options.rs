//! MCP server options

use std::collections::HashMap;
use crate::transport::{StdIo, TransportProto};
use crate::types::{
    Implementation, 
    Tool, ListToolsResult,
    Resource, ListResourcesResult,
    Prompt, ListPromptsResult
};

/// Represents MCP server configuration options
#[derive(Default)]
pub struct McpOptions {
    pub(crate) implementation: Implementation,
    proto: Option<TransportProto>,
    tools: HashMap<String, Tool>,
    resources: HashMap<String, Resource>,
    prompts: HashMap<String, Prompt>,
}

impl McpOptions {
    /// Sets stdio as a transport protocol
    pub fn with_stdio(mut self) -> Self {
        self.proto = Some(TransportProto::Stdio(StdIo::new()));
        self
    }
    
    /// Specifies MCP server name
    pub fn with_server_name(mut self, name: &str) -> Self {
        self.implementation.name = name.into();
        self
    }

    /// Specifies MCP server version
    pub fn with_server_ver(mut self, ver: &str) -> Self {
        self.implementation.version = ver.into();
        self
    }
    
    /// Adds a tool
    pub(crate) fn add_tool(&mut self, tool: Tool) -> &mut Self {
        self.tools.insert(tool.name.clone(), tool);
        self
    }

    /// Adds a resource
    pub(crate) fn add_resource(&mut self, resource: Resource) -> &mut Self {
        self.resources.insert(resource.uri.clone(), resource);
        self
    }

    /// Adds a prompt
    pub(crate) fn add_prompt(&mut self, prompt: Prompt) -> &mut Self {
        self.prompts.insert(prompt.name.clone(), prompt);
        self
    }
    
    /// Returns current transport protocol
    pub(crate) fn transport(&mut self) -> TransportProto {
        let transport = self.proto.take();
        transport.unwrap_or_default()
    }
    
    /// Returns a tool by its name
    #[inline]
    pub(crate) fn get_tool(&self, name: &str) -> Option<&Tool> {
        self.tools.get(name)
    }
    
    /// Returns a list of available tools
    #[inline]
    pub(crate) fn tools(&self) -> ListToolsResult {
        self.tools
            .iter().map(|(_, tool)| tool)
            .collect::<Vec<_>>()
            .into()
    }

    /// Reads a resource by it URI
    #[inline]
    pub(crate) fn read_resource(&self, uri: &str) -> Option<&Resource> {
        self.resources.get(uri)
    }

    /// Returns a list of available resources
    #[inline]
    pub(crate) fn resources(&self) -> ListResourcesResult {
        self.resources
            .iter().map(|(_, tool)| tool)
            .collect::<Vec<_>>()
            .into()
    }

    /// Returns a tool by its name
    #[inline]
    pub(crate) fn get_prompt(&self, name: &str) -> Option<&Prompt> {
        self.prompts.get(name)
    }

    /// Returns a list of available prompts
    #[inline]
    pub(crate) fn prompts(&self) -> ListPromptsResult {
        self.prompts
            .iter().map(|(_, tool)| tool)
            .collect::<Vec<_>>()
            .into()
    }
}

#[cfg(test)]
mod tests {
    
}
//! MCP server options

use std::collections::HashMap;
use crate::transport::{StdIo, TransportProto};
use crate::types::Tool;

/// Represents MCP server configuration options
#[derive(Default)]
pub struct McpOptions {
    proto: Option<TransportProto>,
    tools: HashMap<String, Tool>,
    //prompts: HashMap<&'static str, Prompt>,
    //resources: HashMap<&'static str, Resource>,
}

impl McpOptions {
    /// Sets stdio as a transport protocol
    pub fn with_stdio(mut self) -> Self {
        self.proto = Some(TransportProto::Stdio(StdIo::new()));
        self
    }
    
    /// Adds a tool
    pub(crate) fn add_tool(&mut self, tool: Tool) -> &mut Self {
        self.tools.insert(tool.name.clone(), tool);
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
    pub(crate) fn tools(&self) -> Vec<&Tool> {
        self
            .tools
            .iter().map(|(_, tool)| tool)
            .collect::<Vec<_>>()
    }
}

#[cfg(test)]
mod tests {
    
}
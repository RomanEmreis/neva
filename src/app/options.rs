//! MCP server options

use std::collections::HashMap;
use crate::types::Tool;

/// Represents MCP server configuration options
#[derive(Default)]
pub struct McpOptions {
    pub(super) tools: HashMap<String, Tool>,
    //prompts: HashMap<&'static str, Prompt>,
    //resources: HashMap<&'static str, Resource>,
}

impl McpOptions {
    pub(crate) fn add_tool(&mut self, tool: Tool) -> &mut Self {
        self.tools.insert(tool.name.clone(), tool);
        self
    }
}

#[cfg(test)]
mod tests {
    
}
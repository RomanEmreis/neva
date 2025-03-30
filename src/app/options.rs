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
    /// Information of current server's implementation
    pub(crate) implementation: Implementation,
    
    /// Current transport protocol that server uses
    proto: Option<TransportProto>,
    
    /// A map of tools, where the _key_ is a tool _name_
    tools: HashMap<String, Tool>,

    /// A map of resources, where the _key_ is a resource _URI_
    resources: HashMap<String, Resource>,

    /// A map of prompts, where the _key_ is a prompt _name_
    prompts: HashMap<String, Prompt>,
}

impl McpOptions {
    /// Sets stdio as a transport protocol
    pub fn with_stdio(mut self) -> Self {
        self.proto = Some(TransportProto::Stdio(StdIo::new()));
        self
    }
    
    /// Specifies MCP server name
    pub fn with_name(mut self, name: &str) -> Self {
        self.implementation.name = name.into();
        self
    }

    /// Specifies MCP server version
    pub fn with_version(mut self, ver: &str) -> Self {
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
            .values()
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
            .values()
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
            .values()
            .collect::<Vec<_>>()
            .into()
    }
}

#[cfg(test)]
mod tests {
    use crate::SERVER_NAME;
    use super::*;
    
    #[test]
    fn it_creates_default_options() {
        let options = McpOptions::default();
        
        assert_eq!(options.implementation.name, SERVER_NAME);
        assert_eq!(options.implementation.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(options.tools.len(), 0);
        assert_eq!(options.resources.len(), 0);
        assert_eq!(options.prompts.len(), 0);
        assert!(options.proto.is_none());
    }

    #[test]
    fn it_takes_none_transport_by_default() {
        let mut options = McpOptions::default();
        
        let transport = options.transport();
        
        assert!(matches!(transport, TransportProto::None));
    }
    
    #[test]
    fn it_sets_and_takes_stdio_transport() {
        let mut options = McpOptions::default()
            .with_stdio();
        
        let transport = options.transport();

        assert!(matches!(transport, TransportProto::Stdio(_)));
    }
    
    #[test]
    fn it_sets_server_name() {
        let options = McpOptions::default()
            .with_name("name");
        
        assert_eq!(options.implementation.name, "name");
    }

    #[test]
    fn it_sets_server_version() {
        let options = McpOptions::default()
            .with_version("1");

        assert_eq!(options.implementation.version, "1");
    }
    
    #[test]
    fn it_adds_and_gets_tool() {
        let mut options = McpOptions::default();
        
        options.add_tool(Tool::new("tool", || async { "test" }));
        
        let tool = options.get_tool("tool").unwrap();
        assert_eq!(tool.name, "tool");
    }

    #[test]
    fn it_returns_tools() {
        let mut options = McpOptions::default();

        options.add_tool(Tool::new("tool", || async { "test" }));

        let tools = options.tools();
        assert_eq!(tools.tools.len(), 1);
    }

    #[test]
    fn it_adds_and_reads_resource() {
        let mut options = McpOptions::default();

        options.add_resource(Resource::new("/res", "resource"));

        let res = options.read_resource("/res").unwrap();
        assert_eq!(res.name, "resource");
    }

    #[test]
    fn it_returns_resources() {
        let mut options = McpOptions::default();

        options.add_resource(Resource::new("/res", "resource"));

        let resources = options.resources();
        assert_eq!(resources.resources.len(), 1);
    }

    #[test]
    fn it_adds_and_gets_prompt() {
        let mut options = McpOptions::default();

        options.add_prompt(Prompt::new("test"));

        let prompt = options.get_prompt("test").unwrap();
        assert_eq!(prompt.name, "test");
    }

    #[test]
    fn it_returns_prompts() {
        let mut options = McpOptions::default();

        options.add_prompt(Prompt::new("test"));

        let prompts = options.prompts();
        assert_eq!(prompts.prompts.len(), 1);
    }
}
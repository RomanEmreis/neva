//! MCP server options

use crate::transport::{StdIo, TransportProto};
use crate::app::handler::RequestHandler;
use std::{
    borrow::Cow,
    collections::HashMap
};
use crate::PROTOCOL_VERSIONS;
use crate::types::{
    Implementation, 
    Tool, ListToolsResult, 
    Resource, Uri, ReadResourceResult, ResourceTemplate, ListResourcesResult,
    ListResourceTemplatesResult, resource::Route,
    Prompt, ListPromptsResult,
    ResourcesCapability, ToolsCapability, PromptsCapability
};

/// Represents MCP server configuration options
#[derive(Default)]
pub struct McpOptions {
    /// Information of current server's implementation
    pub(crate) implementation: Implementation,

    /// Tools capability options
    pub(crate) tools_capability: ToolsCapability,

    /// Resource capability options
    pub(crate) resources_capability: ResourcesCapability,

    /// Prompts capability options
    pub(crate) prompts_capability: PromptsCapability,

    /// An MCP version that server supports
    protocol_ver: Option<&'static str>,
    
    /// Current transport protocol that server uses
    proto: Option<TransportProto>,
    
    /// A map of tools, where the _key_ is a tool _name_
    tools: HashMap<String, Tool>,

    /// A map of resources, where the _key_ is a resource name
    resources: HashMap<String, Resource>,

    /// A flat map of resource templates, where the _key_ is a resource template name
    resources_templates: HashMap<String, ResourceTemplate>,
    
    /// A resource template routing data structure
    resource_routes: Route,

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

    /// Specifies Model Context Protocol version
    /// 
    /// Default: last available protocol version
    pub fn with_mcp_version(mut self, ver: &'static str) -> Self {
        self.protocol_ver = Some(ver);
        self
    }

    /// Configures tools capability
    pub fn with_tools<F>(mut self, config: F) -> Self 
    where 
        F: FnOnce(ToolsCapability) -> ToolsCapability
    {
        self.tools_capability = config(self.tools_capability);
        self
    }

    /// Configures resources capability
    pub fn with_resources<F>(mut self, config: F) -> Self
    where
        F: FnOnce(ResourcesCapability) -> ResourcesCapability
    {
        self.resources_capability = config(self.resources_capability);
        self
    }

    /// Configures prompts capability
    pub fn with_prompts<F>(mut self, config: F) -> Self
    where
        F: FnOnce(PromptsCapability) -> PromptsCapability
    {
        self.prompts_capability = config(self.prompts_capability);
        self
    }
    
    /// Adds a tool
    pub(crate) fn add_tool(&mut self, tool: Tool) -> &mut Tool {
        self.tools
            .entry(tool.name.clone())
            .or_insert(tool)
    }

    /// Adds a resource
    pub(crate) fn add_resource(&mut self, resource: Resource) -> &mut Resource {
        self.resources
            .entry(resource.name.clone())
            .or_insert(resource)
    }

    /// Adds a resource template
    pub(crate) fn add_resource_template(
        &mut self, 
        template: ResourceTemplate, 
        handler: RequestHandler<ReadResourceResult>
    ) -> &mut ResourceTemplate {
        let uri_parts: Vec<Cow<'static, str>> = template
            .uri_template
            .as_vec();
        
        self.resource_routes.insert(uri_parts.as_slice(), handler);
        self.resources_templates
            .entry(template.name.clone())
            .or_insert(template.clone())
    }

    /// Adds a prompt
    pub(crate) fn add_prompt(&mut self, prompt: Prompt) -> &mut Prompt {
        self.prompts
            .entry(prompt.name.clone())
            .or_insert(prompt)
    }
    
    /// Returns a Model Context Protocol version that server supports
    #[inline]
    pub(crate) fn protocol_ver(&self) -> &'static str {
        match self.protocol_ver { 
            Some(ver) => ver,
            None => PROTOCOL_VERSIONS.last().unwrap()
        }
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
            .cloned()
            .collect::<Vec<_>>()
            .into()
    }

    /// Reads a resource by it URI
    #[inline]
    pub(crate) fn read_resource(&self, uri: &Uri) -> Option<&Route> {
        let uri_parts = uri.as_vec();
        self.resource_routes
            .find(uri_parts.as_slice())
    }

    /// Returns a list of available resources
    #[inline]
    pub(crate) fn resources(&self) -> ListResourcesResult {
        self.resources
            .values()
            .cloned()
            .collect::<Vec<_>>()
            .into()
    }

    /// Returns a list of available resource templates
    #[inline]
    pub(crate) fn resource_templates(&self) -> ListResourceTemplatesResult {
        self.resources_templates
            .values()
            .cloned()
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
            .cloned()
            .collect::<Vec<_>>()
            .into()
    }
}

#[cfg(test)]
mod tests {
    use crate::error::{Error, ErrorCode};
    use crate::SERVER_NAME;
    use crate::types::resource::template::ResourceFunc;
    use crate::types::resource::Uri;
    use crate::types::{GetPromptRequestParams, PromptMessage, ReadResourceRequestParams, ResourceContents, Role};
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
    fn it_returns_resources() {
        let mut options = McpOptions::default();

        options.add_resource(Resource::new("res://res", "res"));

        let resources = options.resources();
        assert_eq!(resources.resources.len(), 1);
    }

    #[tokio::test]
    async fn it_adds_and_reads_resource_template() {
        let mut options = McpOptions::default();

        let handler = |uri: Uri| async move {
            ResourceContents::text(&uri, "text/plain", "some text")
        };
        
        options.add_resource_template(
            ResourceTemplate::new("res://res", "test"),
            ResourceFunc::new(handler));

        let req = ReadResourceRequestParams {
            uri: "res://res".into()
        };
        
        let res = options.read_resource(&req.uri).unwrap();
        let res = match res { 
            Route::Handler(handler) => handler.call(req.into()).await.unwrap(),
            _ => unreachable!()
        };
        assert_eq!(res.contents.len(), 1);
    }

    #[tokio::test]
    async fn it_adds_and_reads_resource_template_with_err() {
        let mut options = McpOptions::default();

        let handler = |_: Uri| async move {
            Err::<ResourceContents, _>(Error::from(ErrorCode::ResourceNotFound))
        };

        options.add_resource_template(
            ResourceTemplate::new("res://res", "test"),
            ResourceFunc::new(handler));

        let req = ReadResourceRequestParams {
            uri: "res://res".into()
        };

        let res = options.read_resource(&req.uri).unwrap();
        let res = match res {
            Route::Handler(handler) => handler.call(req.into()).await,
            _ => unreachable!()
        };
        assert!(res.is_err());
    }

    #[test]
    fn it_returns_resource_templates() {
        let mut options = McpOptions::default();

        let handler = |uri: Uri| async move {
            ResourceContents::text(&uri, "text/plain", "some text")
        };

        options.add_resource_template(
            ResourceTemplate::new("res://res", "test"),
            ResourceFunc::new(handler));

        let resources = options.resource_templates();
        assert_eq!(resources.templates.len(), 1);
    }

    #[tokio::test]
    async fn it_adds_and_gets_prompt() {
        let mut options = McpOptions::default();

        options.add_prompt(Prompt::new("test", || async { 
            [("test", Role::User)]
        }));

        let prompt = options.get_prompt("test").unwrap();
        assert_eq!(prompt.name, "test");

        let req = GetPromptRequestParams {
            name: "test".into(),
            args: None
        };

        let result = prompt.call(req.into()).await.unwrap();

        let msg = result.messages.first().unwrap();

        assert_eq!(msg.role, Role::User)
    }

    #[tokio::test]
    async fn it_adds_and_gets_prompt_with_error() {
        let mut options = McpOptions::default();

        options.add_prompt(Prompt::new("test", || async { 
            Err::<PromptMessage, _>(Error::from(ErrorCode::InternalError))
        }));

        let prompt = options.get_prompt("test").unwrap();
        assert_eq!(prompt.name, "test");

        let req = GetPromptRequestParams {
            name: "test".into(),
            args: None
        };

        let result = prompt.call(req.into()).await;

        assert!(result.is_err())
    }

    #[test]
    fn it_returns_prompts() {
        let mut options = McpOptions::default();

        options.add_prompt(Prompt::new("test", || async {
            [("test", Role::User)]
        }));

        let prompts = options.prompts();
        assert_eq!(prompts.prompts.len(), 1);
    }
}
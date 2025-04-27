//! MCP server options

use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use crate::transport::{StdIo, TransportProto};
use crate::app::handler::RequestHandler;
use std::{
    borrow::Cow,
    collections::HashMap
};
use crate::PROTOCOL_VERSIONS;
use crate::types::{
    RequestId,
    Implementation, 
    Tool, 
    Resource, Uri, ReadResourceResult, ResourceTemplate,
    resource::Route,
    Prompt,
    ResourcesCapability, ToolsCapability, PromptsCapability,
};

#[cfg(feature = "tracing")]
use crate::error::Error;
#[cfg(feature = "tracing")]
use crate::types::notification::LoggingLevel;

#[cfg(feature = "tracing")]
use tracing_subscriber::{
    filter::LevelFilter, 
    reload::Handle, 
    Registry
};

#[cfg(feature = "tracing")]
use crate::error::ErrorCode;

/// Represents MCP server options that are available in runtime
pub type RuntimeMcpOptions = Arc<McpOptions>;

/// Represents MCP server configuration options
#[derive(Default)]
pub struct McpOptions {
    /// Information of current server's implementation
    pub(crate) implementation: Implementation,

    /// Tools capability options
    tools_capability: Option<ToolsCapability>,

    /// Resource capability options
    resources_capability: Option<ResourcesCapability>,

    /// Prompts capability options
    prompts_capability: Option<PromptsCapability>,
    
    /// The last logging level set by the client
    #[cfg(feature = "tracing")]
    log_level: Option<Handle<LevelFilter, Registry>>,

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
    
    /// Currently running requests
    requests: RwLock<HashMap<RequestId, CancellationToken>>
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
        self.tools_capability = Some(config(Default::default()));
        self
    }

    /// Configures resources capability
    pub fn with_resources<F>(mut self, config: F) -> Self
    where
        F: FnOnce(ResourcesCapability) -> ResourcesCapability
    {
        self.resources_capability = Some(config(Default::default()));
        self
    }

    /// Configures prompts capability
    pub fn with_prompts<F>(mut self, config: F) -> Self
    where
        F: FnOnce(PromptsCapability) -> PromptsCapability
    {
        self.prompts_capability = Some(config(Default::default()));
        self
    }
    
    /// Configures [`LogLevelHandle`] that allow to change the [`LoggingLevel`] in runtime
    #[cfg(feature = "tracing")]
    pub fn with_logging(mut self, log_handle: Handle<LevelFilter, Registry>) -> Self {
        self.log_level = Some(log_handle);
        self
    }
    
    /// Sets the [`LoggingLevel`]
    #[cfg(feature = "tracing")]
    pub fn set_log_level(&self, level: LoggingLevel) -> Result<(), Error> {
        if let Some(handle) = &self.log_level {
            handle
                .modify(|current| *current = level.into())
                .map_err(|e| Error::new(ErrorCode::InternalError, e.to_string()))?;
        }
        Ok(())
    }

    /// Returns current log level
    #[cfg(feature = "tracing")]
    pub(crate) fn log_level(&self) -> Option<LoggingLevel> {
        match &self.log_level { 
            None => None,
            Some(handle) => handle
                .clone_current()
                .map(|x| x.into()),
        }
    }
    
    /// Tracks the request with `req_id` and returns the [`CancellationToken`] for this request
    pub(crate) async fn track_request(&self, req_id: &RequestId) -> CancellationToken {
        let token = CancellationToken::new();

        let mut requests = self.requests.write().await;
        requests.insert(req_id.clone(), token.clone());
        
        token
    }
    
    /// Cancels the request with `req_id` if it is present
    pub(crate) async fn cancel_request(&self, req_id: &RequestId) {
        if let Some(token) = self.requests.write().await.remove(req_id) {
            token.cancel();
        }
    }

    /// Completes the request with `req_id` if it is present
    pub(crate) async fn complete_request(&self, req_id: &RequestId) {
        self.requests.write()
            .await
            .remove(req_id);
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
    pub(crate) fn tools(&self) -> Vec<Tool> {
        self.tools
            .values()
            .cloned()
            .collect()
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
    pub(crate) fn resources(&self) -> Vec<Resource> {
        self.resources
            .values()
            .cloned()
            .collect()
    }

    /// Returns a list of available resource templates
    #[inline]
    pub(crate) fn resource_templates(&self) -> Vec<ResourceTemplate> {
        self.resources_templates
            .values()
            .cloned()
            .collect()
    }

    /// Returns a tool by its name
    #[inline]
    pub(crate) fn get_prompt(&self, name: &str) -> Option<&Prompt> {
        self.prompts.get(name)
    }

    /// Returns a list of available prompts
    #[inline]
    pub(crate) fn prompts(&self) -> Vec<Prompt> {
        self.prompts
            .values()
            .cloned()
            .collect()
    }

    /// Returns [`ToolsCapability`] if configured.
    /// If not configured but at least one [`Tool`] exists, returns [`Default`].
    /// Otherwise, returns `None`.
    pub(crate) fn tools_capability(&self) -> Option<ToolsCapability> {
        self.tools_capability
            .clone()
            .or_else(|| (!self.tools.is_empty()).then(Default::default))
    }

    /// Returns [`ResourcesCapability`] if configured.
    /// If not configured but at least one [`Resource`] or [`ResourceTemplate`] exists, returns [`Default`].
    /// Otherwise, returns `None`.
    pub(crate) fn resources_capability(&self) -> Option<ResourcesCapability> {
        self.resources_capability
            .clone()
            .or_else(
                || (!self.resources.is_empty() 
                || !self.resources_templates.is_empty()).then(Default::default))
    }

    /// Returns [`PromptsCapability`] if configured.
    /// If not configured but at least one [`Prompt`] exists, returns [`Default`].
    /// Otherwise, returns `None`.
    pub(crate) fn prompts_capability(&self) -> Option<PromptsCapability> {
        self.prompts_capability
            .clone()
            .or_else(|| (!self.prompts.is_empty()).then(Default::default))
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
        assert_eq!(tools.len(), 1);
    }

    #[test]
    fn it_returns_resources() {
        let mut options = McpOptions::default();

        options.add_resource(Resource::new("res://res", "res"));

        let resources = options.resources();
        assert_eq!(resources.len(), 1);
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
            uri: "res://res".into(),
            meta: None
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
            uri: "res://res".into(),
            meta: None
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
        assert_eq!(resources.len(), 1);
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
            args: None,
            meta: None,
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
            args: None,
            meta: None,
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
        assert_eq!(prompts.len(), 1);
    }
    
    #[test]
    fn it_returns_some_tool_capabilities_if_configured() {
        let options = McpOptions::default()
            .with_tools(|tools| tools.with_list_changed());
        
        let tools_capability = options.tools_capability().unwrap();
        
        assert!(tools_capability.list_changed);
    }

    #[test]
    fn it_returns_some_tool_capabilities_if_there_are_tools() {
        let mut options = McpOptions::default();
        options.add_tool(Tool::new("tool", || async { "test" }));
        
        let tools_capability = options.tools_capability().unwrap();

        assert!(!tools_capability.list_changed);
    }

    #[test]
    fn it_returns_none_tool_capabilities() {
        let options = McpOptions::default();

        assert!(options.tools_capability().is_none());
    }

    #[test]
    fn it_returns_some_resource_capabilities_if_configured() {
        let options = McpOptions::default()
            .with_resources(|res| res.with_list_changed());

        let resources_capability = options.resources_capability().unwrap();

        assert!(resources_capability.list_changed);
    }

    #[test]
    fn it_returns_some_resources_capability_if_there_are_resources() {
        let mut options = McpOptions::default();
        options.add_resource(Resource::new("res", "test"));

        let resources_capability = options.resources_capability().unwrap();

        assert!(!resources_capability.list_changed);
    }

    #[test]
    fn it_returns_some_resources_capability_if_there_are_resource_templates() {
        let mut options = McpOptions::default();

        let handler = |_: Uri| async move {
            Err::<ResourceContents, _>(Error::from(ErrorCode::ResourceNotFound))
        };
        
        options.add_resource_template(
            ResourceTemplate::new("res", "test"), 
            ResourceFunc::new(handler));

        let resources_capability = options.resources_capability().unwrap();

        assert!(!resources_capability.list_changed);
    }

    #[test]
    fn it_returns_none_resources_capability() {
        let options = McpOptions::default();

        assert!(options.resources_capability().is_none());
    }

    #[test]
    fn it_returns_some_prompts_capability_if_configured() {
        let options = McpOptions::default()
            .with_prompts(|prompts| prompts.with_list_changed());

        let prompts_capability = options.prompts_capability().unwrap();

        assert!(prompts_capability.list_changed);
    }

    #[test]
    fn it_returns_some_prompts_capability_if_there_are_tools() {
        let mut options = McpOptions::default();
        options.add_prompt(Prompt::new("test", || async {
            Err::<PromptMessage, _>(Error::from(ErrorCode::InternalError))
        }));

        let prompts_capability = options.prompts_capability().unwrap();

        assert!(!prompts_capability.list_changed);
    }

    #[test]
    fn it_returns_none_prompts_capability() {
        let options = McpOptions::default();

        assert!(options.prompts_capability().is_none());
    }
}
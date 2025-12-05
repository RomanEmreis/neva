//! MCP server options

use dashmap::{DashMap, DashSet};
use std::{sync::Arc, time::Duration};
use std::fmt::{Debug, Formatter};
use tokio_util::sync::CancellationToken;
use crate::transport::{StdIoServer, TransportProto};
#[cfg(feature = "http-server")]
use crate::transport::HttpServer;
use crate::app::{handler::RequestHandler, collection::Collection};

use crate::middleware::{Middleware, Middlewares};

use crate::PROTOCOL_VERSIONS;
use crate::types::{
    RequestId,
    Implementation,
    Tool,
    Resource, Uri, ReadResourceResult, ResourceTemplate,
    resource::{Route, route::ResourceHandler},
    Prompt,
    ResourcesCapability, ToolsCapability, PromptsCapability
};

#[cfg(feature = "tracing")]
use crate::types::notification::LoggingLevel;
#[cfg(feature = "tasks")]
use crate::types::{
    ServerTasksCapability,
    CallToolResponse,
    TaskPayload,
    Task,
};

#[cfg(feature = "tracing")]
use tracing_subscriber::{
    filter::LevelFilter, 
    reload::Handle, 
    Registry
};

#[cfg(any(feature = "tracing", feature = "tasks"))]
use crate::error::{Error, ErrorCode};

/// Represents MCP server options that are available in runtime
pub type RuntimeMcpOptions = Arc<McpOptions>;

/// Represents MCP server configuration options
pub struct McpOptions {
    /// Information of current server's implementation
    pub(crate) implementation: Implementation,
    
    /// Timeout for the requests from server to a client
    pub(crate) request_timeout: Duration,

    /// A map of tools, where the _key_ is a tool _name_
    pub(super) tools: Collection<Tool>,

    /// A map of prompts, where the _key_ is a prompt _name_
    pub(super) prompts: Collection<Prompt>,

    /// A map of resources, where the _key_ is a resource name
    pub(super) resources: Collection<Resource>,

    /// A flat map of resource templates, where the _key_ is a resource template name
    pub(super) resources_templates: Collection<ResourceTemplate>,

    /// Holds current subscriptions to resource changes
    pub(super) resource_subscriptions: DashSet<Uri>,
    
    /// An ordered list of middlewares
    pub(super) middlewares: Option<Middlewares>,

    /// Tools capability options
    tools_capability: Option<ToolsCapability>,

    /// Resource capability options
    resources_capability: Option<ResourcesCapability>,

    /// Prompts capability options
    prompts_capability: Option<PromptsCapability>,

    /// Server tasks capability options
    #[cfg(feature = "tasks")]
    tasks_capability: Option<ServerTasksCapability>,
    
    /// The last logging level set by the client
    #[cfg(feature = "tracing")]
    log_level: Option<Handle<LevelFilter, Registry>>,

    /// An MCP version that server supports
    protocol_ver: Option<&'static str>,
    
    /// Current transport protocol that this server uses
    proto: Option<TransportProto>,

    /// A resource template routing data structure
    resource_routes: Route,
    
    /// Currently running requests
    requests: DashMap<RequestId, CancellationToken>,

    /// Currently running tasks
    #[cfg(feature = "tasks")]
    tasks: DashMap<String, TaskEntry>
}

/// Represents a task currently running on the server
#[cfg(feature = "tasks")]
pub(crate) struct TaskEntry {
    task: Task,
    token: CancellationToken,
    result_tx: tokio::sync::watch::Sender<Option<TaskPayload<CallToolResponse>>>,
}

#[cfg(feature = "tasks")]
pub(crate) struct TaskHandle {
    pub(crate) token: CancellationToken,
    pub(crate) result_tx: tokio::sync::watch::Sender<Option<TaskPayload<CallToolResponse>>>,
}

impl Debug for McpOptions {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut binding = f.debug_struct("McpOptions");
        let dbg = binding
            .field("implementation", &self.implementation)
            .field("request_timeout", &self.request_timeout)
            .field("tools_capability", &self.tools_capability)
            .field("resources_capability", &self.resources_capability)
            .field("prompts_capability", &self.prompts_capability)
            .field("protocol_ver", &self.protocol_ver);
        
        #[cfg(feature = "tasks")]
        dbg.field("server_tasks_capability", &self.tasks_capability);
        
        #[cfg(feature = "tracing")]
        dbg.field("log_level", &self.log_level);
        
        dbg.finish()
    }
}

impl Default for McpOptions {
    #[inline]
    fn default() -> Self {
        Self {
            implementation: Default::default(),
            request_timeout: Duration::from_secs(10),
            tools: Collection::new(),
            resources: Collection::new(),
            prompts: Collection::new(),
            resources_templates: Collection::new(),
            proto: Default::default(),
            protocol_ver: Default::default(),
            tools_capability: Default::default(),
            resources_capability: Default::default(),
            prompts_capability: Default::default(),
            #[cfg(feature = "tasks")]
            tasks_capability: Default::default(),
            resource_routes: Default::default(),
            requests: Default::default(),
            resource_subscriptions: Default::default(),
            middlewares: None,
            #[cfg(feature = "tracing")]
            log_level: Default::default(),
            #[cfg(feature = "tasks")]
            tasks: Default::default(),
        }
    }
}

impl McpOptions {
    /// Sets stdio as a transport protocol
    pub fn with_stdio(mut self) -> Self {
        self.proto = Some(TransportProto::StdIoServer(StdIoServer::new()));
        self
    }

    /// Sets Streamable HTTP as a transport protocol
    #[cfg(feature = "http-server")]
    pub fn set_http(mut self, http: HttpServer) -> Self {
        self.proto = Some(TransportProto::HttpServer(Box::new(http)));
        self
    }
    
    /// Sets Streamable HTTP as a transport protocol
    #[cfg(feature = "http-server")]
    pub fn with_http<F: FnOnce(HttpServer) -> HttpServer>(mut self, config: F) -> Self {
        self.proto = Some(TransportProto::HttpServer(Box::new(config(HttpServer::default()))));
        self
    }

    /// Sets Streamable HTTP as a transport protocol with default configuration
    /// 
    /// Default:
    /// * __IP__: 127.0.0.1
    /// * __PORT__: 3000
    /// * __ENDPOINT__: /mcp
    #[cfg(feature = "http-server")]
    pub fn with_default_http(self) -> Self {
        self.with_http(|http| http)
    }
    
    /// Specifies MCP server name
    pub fn with_name(mut self, name: &str) -> Self {
        self.implementation.name = name.into();
        self
    }

    /// Specifies the MCP server version
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

    /// Configures tasks capability
    #[cfg(feature = "tasks")]
    pub fn with_tasks<F>(mut self, config: F) -> Self
    where
        F: FnOnce(ServerTasksCapability) -> ServerTasksCapability
    {
        self.tasks_capability = Some(config(Default::default()));
        self
    }

    /// Specifies request timeout
    ///
    /// Default: 10 seconds
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
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
    pub(crate) fn track_request(&self, req_id: &RequestId) -> CancellationToken {
        let token = CancellationToken::new();
        self.requests.insert(req_id.clone(), token.clone());
        token
    }
    
    /// Cancels the request with `req_id` if it is present
    pub(crate) fn cancel_request(&self, req_id: &RequestId) {
        if let Some((_, token)) = self.requests.remove(req_id) {
            token.cancel();
        }
    }

    /// Completes the request with `req_id` if it is present
    pub(crate) fn complete_request(&self, req_id: &RequestId) {
        self.requests
            .remove(req_id);
    }

    /// Tacks the task and returns the [`CancellationToken`] for this task
    #[cfg(feature = "tasks")]
    pub(crate) fn track_task(&self, task: Task) -> TaskHandle {
        let token = CancellationToken::new();
        let (tx, _rx) = tokio::sync::watch::channel(None);
        
        self.tasks.insert(task.id.clone(), TaskEntry {
            result_tx: tx.clone(),
            token: token.clone(),
            task,
        });
        
        TaskHandle {
            result_tx: tx,
            token
        }
    }

    /// Cancels the task
    #[cfg(feature = "tasks")]
    pub(crate) fn cancel_task(&self, task_id: &str) -> Result<Task, Error> {
        if let Some((_, entry)) = self.tasks.remove(task_id) {
            entry.token.cancel();
            Ok(entry.task.cancel())
        } else { 
            Err(Error::new(
                ErrorCode::InvalidParams, 
                format!("Could not find task with id {task_id}")))
        }
    }

    /// Completes the task
    #[cfg(feature = "tasks")]
    pub(crate) fn complete_task(
        &self, 
        task_id: &str, 
        result: Result<CallToolResponse, Error>
    ) {
        if let Some(mut entry) = self.tasks.get_mut(task_id) {
            let result = match result { 
                Ok(result) => TaskPayload(result),
                Err(err) => TaskPayload(CallToolResponse::error(err))
            };
            entry.task.complete();
            let _ = entry.result_tx.send(Some(result));
        }
    }

    /// Retrieves the task status 
    #[cfg(feature = "tasks")]
    pub(crate) fn get_task_status(&self, task_id: &str) -> Result<Task, Error> {
        self.tasks
            .get(task_id)
            .map(|t| t.task.clone())
            .ok_or_else(|| Error::new(
                ErrorCode::InvalidParams, 
                format!("Could not find task with id {task_id}")))
    }

    /// Awaits the task result
    #[cfg(feature = "tasks")]
    pub(crate) async fn get_task_result(&self, task_id: &str) -> Result<TaskPayload<CallToolResponse>, Error> {
        let (mut result_rx, token) = {
            let entry = self.tasks
                .get(task_id)
                .ok_or_else(|| Error::new(
                    ErrorCode::InvalidParams,
                    format!("Could not find task with id {task_id}")))?;
            
            if let Some(ref result) = *entry.result_tx.borrow() {
                return Ok(result.clone());
            }

            (
                entry.result_tx.subscribe(),
                entry.token.clone(),
            )
        };

        loop {
            tokio::select! {
                changed = result_rx.changed() => {
                    if changed.is_err() {
                        return Err(Error::new(ErrorCode::InternalError, "Unable to get task result"));
                    }

                    if let Some(result) = result_rx.borrow_and_update().clone() {
                        return Ok(result);
                    }
                }
                _ = token.cancelled() => {
                    return Err(Error::new(ErrorCode::InvalidRequest, "Task has been cancelled"));
                }
            }
        }
    }
    
    /// Adds a tool
    pub(crate) fn add_tool(&mut self, tool: Tool) -> &mut Tool {
        self.tools_capability
            .get_or_insert_default();

        self.tools
            .as_mut()
            .entry(tool.name.clone())
            .or_insert(tool)
    }

    /// Adds a resource
    pub(crate) fn add_resource(&mut self, resource: Resource) -> &mut Resource {
        self.resources_capability
            .get_or_insert_default();
        
        self.resources
            .as_mut()
            .entry(resource.uri.to_string())
            .or_insert(resource)
    }

    /// Adds a resource template
    pub(crate) fn add_resource_template(
        &mut self, 
        template: ResourceTemplate, 
        handler: RequestHandler<ReadResourceResult>
    ) -> &mut ResourceTemplate {
        self.resources_capability
            .get_or_insert_default();
        
        let name = template.name.clone();
        
        self.resource_routes.insert(&template.uri_template, name.clone(), handler);
        self.resources_templates
            .as_mut()
            .entry(name)
            .or_insert(template)
    }

    /// Adds a prompt
    pub(crate) fn add_prompt(&mut self, prompt: Prompt) -> &mut Prompt {
        self.prompts_capability
            .get_or_insert_default();
        
        self.prompts
            .as_mut()
            .entry(prompt.name.clone())
            .or_insert(prompt)
    }
    
    /// Registers a middleware
    #[inline]
    pub(crate) fn add_middleware(&mut self, middleware: Middleware) {
        self.middlewares
            .get_or_insert_with(Middlewares::new)
            .add(middleware);
    }
    
    /// Returns a Model Context Protocol version that this server supports
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
    pub(crate) async fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.get(name).await
    }
    
    /// Returns a list of available tools
    #[inline]
    pub(crate) async fn list_tools(&self) -> Vec<Tool> {
        self.tools.values().await
    }

    /// Reads a resource by its URI
    #[inline]
    pub(crate) fn read_resource(&self, uri: &Uri) -> Option<(&ResourceHandler, Box<[String]>)> {
        self.resource_routes.find(uri)
    }

    /// Returns a list of available resources
    #[inline]
    pub(crate) async fn list_resources(&self) -> Vec<Resource> {
        self.resources.values().await
    }

    /// Returns a list of available resource templates
    #[inline]
    pub(crate) async fn list_resource_templates(&self) -> Vec<ResourceTemplate> {
        self.resources_templates.values().await
    }

    /// Returns a tool by its name
    #[inline]
    pub(crate) async fn get_prompt(&self, name: &str) -> Option<Prompt> {
        self.prompts.get(name).await
    }

    /// Returns a list of available prompts
    #[inline]
    pub(crate) async fn list_prompts(&self) -> Vec<Prompt> {
        self.prompts.values().await
    }

    /// Returns [`ToolsCapability`] if configured.
    /// If not configured but at least one [`Tool`] exists, returns [`Default`].
    /// Otherwise, returns `None`.
    pub(crate) fn tools_capability(&self) -> Option<ToolsCapability> {
        self.tools_capability.clone()
    }

    /// Returns [`ResourcesCapability`] if configured.
    /// If not configured but at least one [`Resource`] or [`ResourceTemplate`] exists, returns [`Default`].
    /// Otherwise, returns `None`.
    pub(crate) fn resources_capability(&self) -> Option<ResourcesCapability> {
        self.resources_capability.clone()
    }

    /// Returns [`PromptsCapability`] if configured.
    /// If not configured but at least one [`Prompt`] exists, returns [`Default`].
    /// Otherwise, returns `None`.
    pub(crate) fn prompts_capability(&self) -> Option<PromptsCapability> {
        self.prompts_capability.clone()
    }

    /// Returns [`ServerTasksCapability`] if configured.
    /// 
    /// Otherwise, returns `None`.
    #[cfg(feature = "tasks")]
    pub(crate) fn tasks_capability(&self) -> Option<ServerTasksCapability> {
        self.tasks_capability.clone()
    }

    /// Returns whether the server is configured to send the "notifications/resources/updated"
    #[inline]
    pub(crate) fn is_resource_subscription_supported(&self) -> bool {
        self.resources_capability
            .as_ref()
            .is_some_and(|res| res.subscribe)
    }

    /// Returns whether the server is configured to send the "notifications/resources/list_changed"
    #[inline]
    pub(crate) fn is_resource_list_changed_supported(&self) -> bool {
        self.resources_capability
            .as_ref()
            .is_some_and(|res| res.list_changed)
    }

    /// Returns whether the server is configured to send the "notifications/tools/list_changed"
    #[inline]
    pub(crate) fn is_tools_list_changed_supported(&self) -> bool {
        self.tools_capability
            .as_ref()
            .is_some_and(|tool| tool.list_changed)
    }

    /// Returns whether the server is configured to send the "notifications/prompts/list_changed"
    #[inline]
    pub(crate) fn is_prompts_list_changed_supported(&self) -> bool {
        self.prompts_capability
            .as_ref()
            .is_some_and(|prompt| prompt.list_changed)
    }

    /// Returns whether the server is configured to handle the "tasks/list" requests.
    #[inline]
    #[cfg(feature = "tasks")]
    pub(crate) fn is_tasks_list_supported(&self) -> bool {
        self.tasks_capability
            .as_ref()
            .is_some_and(|tasks| tasks.list.is_some())
    }

    /// Returns whether the server is configured to handle the "tasks/cancel" requests.
    #[inline]
    #[cfg(feature = "tasks")]
    pub(crate) fn is_tasks_cancellation_supported(&self) -> bool {
        self.tasks_capability
            .as_ref()
            .is_some_and(|tasks| tasks.cancel.is_some())
    }

    /// Returns whether the server is configured to handle the task-augmented "tools/call" requests.
    #[inline]
    #[cfg(feature = "tasks")]
    pub(crate) fn is_task_augmented_tool_call_supported(&self) -> bool {
        self.tasks_capability
            .as_ref()
            .and_then(|tasks| tasks.requests.as_ref())
            .and_then(|req| req.tools.as_ref())
            .is_some_and(|tools| tools.call.is_some())
    }
    
    /// Turns [`McpOptions`] into [`RuntimeMcpOptions`]
    pub(crate) fn into_runtime(mut self) -> RuntimeMcpOptions {
        self.tools = self.tools.into_runtime();
        self.prompts = self.prompts.into_runtime();
        self.resources = self.resources.into_runtime();
        self.resources_templates = self.resources_templates.into_runtime();
        Arc::new(self)
    }
}

#[cfg(test)]
mod tests {
    use crate::error::{Error, ErrorCode};
    use crate::SDK_NAME;
    use crate::types::resource::template::ResourceFunc;
    use crate::types::resource::Uri;
    use crate::types::{GetPromptRequestParams, PromptMessage, ReadResourceRequestParams, ResourceContents, Role};
    use super::*;
    
    #[test]
    fn it_creates_default_options() {
        let options = McpOptions::default();
        
        assert_eq!(options.implementation.name, SDK_NAME);
        assert_eq!(options.implementation.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(options.tools.as_ref().len(), 0);
        assert_eq!(options.resources.as_ref().len(), 0);
        assert_eq!(options.resources_templates.as_ref().len(), 0);
        assert_eq!(options.prompts.as_ref().len(), 0);
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

        assert!(matches!(transport, TransportProto::StdIoServer(_)));
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
    
    #[tokio::test]
    async fn it_adds_and_gets_tool() {
        let mut options = McpOptions::default();
        
        options.add_tool(Tool::new("tool", || async { "test" }));
        
        let tool = options.get_tool("tool").await.unwrap();
        assert_eq!(tool.name, "tool");
    }

    #[tokio::test]
    async fn it_returns_tools() {
        let mut options = McpOptions::default();

        options.add_tool(Tool::new("tool", || async { "test" }));

        let tools = options.list_tools().await;
        assert_eq!(tools.len(), 1);
    }

    #[tokio::test]
    async fn it_returns_resources() {
        let mut options = McpOptions::default();

        options.add_resource(Resource::new("res://res", "res"));

        let resources = options.list_resources().await;
        assert_eq!(resources.len(), 1);
    }

    #[tokio::test]
    async fn it_adds_and_reads_resource_template() {
        let mut options = McpOptions::default();

        let handler = |uri: Uri| async move {
            ResourceContents::new(uri)
                .with_mime("text/plain")
                .with_text("some text")
        };
        
        options.add_resource_template(
            ResourceTemplate::new("res://res", "test"),
            ResourceFunc::new(handler));

        let req = ReadResourceRequestParams {
            uri: "res://res".into(),
            meta: None,
            args: None
        };
        
        let res = options.read_resource(&req.uri).unwrap();
        let res = res.0.call(req.into()).await.unwrap();
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
            meta: None,
            args: None
        };

        let res = options.read_resource(&req.uri).unwrap();
        let res = res.0.call(req.into()).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn it_returns_resource_templates() {
        let mut options = McpOptions::default();

        let handler = |uri: Uri| async move {
            ResourceContents::new(uri)
                .with_mime("text/plain")
                .with_text("some text")
        };

        options.add_resource_template(
            ResourceTemplate::new("res://res", "test"),
            ResourceFunc::new(handler));

        let resources = options.list_resource_templates().await;
        assert_eq!(resources.len(), 1);
    }

    #[tokio::test]
    async fn it_adds_and_gets_prompt() {
        let mut options = McpOptions::default();

        options.add_prompt(Prompt::new("test", || async { 
            [("test", Role::User)]
        }));

        let prompt = options.get_prompt("test").await.unwrap();
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

        let prompt = options.get_prompt("test").await.unwrap();
        assert_eq!(prompt.name, "test");

        let req = GetPromptRequestParams {
            name: "test".into(),
            args: None,
            meta: None,
        };

        let result = prompt.call(req.into()).await;

        assert!(result.is_err())
    }

    #[tokio::test]
    async fn it_returns_prompts() {
        let mut options = McpOptions::default();

        options.add_prompt(Prompt::new("test", || async {
            [("test", Role::User)]
        }));

        let prompts = options.list_prompts().await;
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
            ResourceTemplate::new("res://test", "test"), 
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
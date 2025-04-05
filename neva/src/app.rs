use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use crate::error::{Error, ErrorCode};
use crate::options::McpOptions;
use crate::transport::Transport;
use crate::app::handler::{
    FromHandlerParams,
    GenericHandler,
    HandlerParams,
    RequestFunc,
    RequestHandler
};
use crate::types::{
    InitializeResult, InitializeRequestParams,
    IntoResponse, Response, 
    CompleteResult, CompleteRequestParams, 
    ListToolsRequestParams, CallToolRequestParams, ListToolsResult, CallToolResponse, Tool, ToolHandler,
    ListResourceTemplatesRequestParams, ListResourceTemplatesResult, ResourceTemplate, 
    ListResourcesRequestParams, ListResourcesResult, ReadResourceRequestParams, ReadResourceResult, 
    SubscribeRequestParams, UnsubscribeRequestParams, Resource, resource::{Route, template::ResourceFunc}, 
    ListPromptsRequestParams, ListPromptsResult, 
    GetPromptRequestParams, GetPromptResult, PromptHandler, Prompt
};

pub mod options;
pub(crate) mod handler;

/// Represents an MCP server application
#[derive(Default)]
pub struct App {
    options: McpOptions,
    handlers: HashMap<String, RequestHandler<Response>>,
}

impl App {
    /// Initializes a new app
    pub fn new() -> App {
        let mut app = Self { 
            options: McpOptions::default(),
            handlers: HashMap::new()
        };

        app.map_handler("initialize", Self::init);
        app.map_handler("completion/complete", Self::completion);
        
        app.map_handler("tools/list", Self::tools);
        app.map_handler("tools/call", Self::tool);
        
        app.map_handler("resources/list", Self::resources);
        app.map_handler("resources/templates/list", Self::resource_templates);
        app.map_handler("resources/read", Self::resource);
        app.map_handler("resources/subscribe", Self::resource_subscribe);
        app.map_handler("resources/unsubscribe", Self::resource_unsubscribe);
        
        app.map_handler("prompts/list", Self::prompts);
        app.map_handler("prompts/get", Self::prompt);
        
        app.map_handler("notifications/initialized", Self::notifications_init);
        app.map_handler("notifications/cancelled", Self::notifications_cancel);
        
        app.map_handler("ping", Self::ping);
        
        app
    }
    
    /// Run the MCP server
    /// 
    /// # Example
    /// ```no_run
    /// use neva::App;
    /// 
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut app = App::new();
    /// 
    /// // configure tools, resources, prompts
    /// 
    /// app.run().await;
    /// # }
    /// ```
    pub async fn run(mut self) {
        let mut transport = self.options.transport();
        let options = Arc::new(self.options);

        transport.start();
        
        while let Ok(req) = transport.recv().await {
            let req_id = req.id();
            let resp = match self.handlers.get(&req.method) {
                Some(handler) => handler.call(HandlerParams::Request(options.clone(), req)).await,
                None => Err(Error::new(ErrorCode::MethodNotFound, "unknown request"))
            };
            match transport.send(resp.into_response(req_id)).await { 
                Ok(_) => (),
                Err(e) => eprintln!("Error sending response: {:?}", e),
            }
        }
    }
    
    /// Configure MCP server options
    pub fn with_options<F>(mut self, config: F) -> Self
    where 
        F: FnOnce(McpOptions) -> McpOptions
    {
        self.options = config(self.options);
        self
    }

    /// Maps an MCP client request to a specific function
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    /// 
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut app = App::new();
    ///
    /// app.map_handler("ping", || async { 
    ///     "pong"
    /// });
    ///
    /// # app.run().await;
    /// # }
    /// ```
    pub fn map_handler<F, R, Args>(&mut self, name: &str, handler: F) -> &mut Self
    where 
        F: GenericHandler<Args, Output = R>,
        R: IntoResponse + Send + 'static,
        Args: FromHandlerParams + Send + Sync + 'static,
    {
        let handler = RequestFunc::new(handler);
        self.handlers.insert(name.into(), handler);
        self
    }

    /// Maps an MCP tool call request to a specific function and returns a mutable reference to the
    /// [`Tool`] for further configuration
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut app = App::new();
    ///
    /// app.map_tool("hello", |name: String| async move { 
    ///     format!("Hello, {name}")
    /// });
    ///
    /// # app.run().await;
    /// # }
    /// ```
    pub fn map_tool<F, R, Args>(&mut self, name: &str, handler: F) -> &mut Tool
    where
        F: ToolHandler<Args, Output = R>,
        R: Into<CallToolResponse> + Send + 'static,
        Args: TryFrom<CallToolRequestParams, Error = Error> + Send + Sync + 'static,
    {
        self.options.add_tool(Tool::new(name, handler))
    }
    
    /// Adds a known resource
    pub fn add_resource(&mut self, uri: &'static str, name: &str) -> &mut Resource {
        let resource = Resource::new(uri, name);
        self.options.add_resource(resource)
    }

    /// Maps an MCP resource read request to a specific function
    ///
    /// # Example
    /// ```no_run
    /// use neva::{App, types::ResourceContents};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// use neva::types::content;
    /// let mut app = App::new();
    ///
    /// app.map_resource("res://{name}", "read_resource", |name: String| async move {
    ///     let content = (format!("res://{name}"), format!("Resource: {name} content"));
    ///     [content]
    /// });
    ///
    /// # app.run().await;
    /// # }
    /// ```
    pub fn map_resource<F, R, Args>(&mut self, uri: &'static str, name: &str, handler: F) -> &mut ResourceTemplate
    where
        F: GenericHandler<Args, Output = R>,
        R: Into<ReadResourceResult> + Send + 'static,
        Args: TryFrom<ReadResourceRequestParams, Error = Error> + Send + Sync + 'static,
    {
        let handler = ResourceFunc::new(handler);
        let template = ResourceTemplate::new(uri, name);
        
        self.options.add_resource_template(template, handler)
    }

    /// Maps an MCP resource read request to a specific function
    ///
    /// # Example
    /// ```no_run
    /// use neva::{App, types::{Resource, ListResourcesRequestParams}};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// use neva::types::content;
    /// let mut app = App::new();
    ///
    /// app.map_resources(|_params: ListResourcesRequestParams| async move {
    ///     [
    ///         Resource::new("res://res1", "res1"),
    ///         Resource::new("res://res2", "res2")
    ///     ]
    /// });
    ///
    /// # app.run().await;
    /// # }
    /// ```
    pub fn map_resources<F, R>(&mut self, handler: F) -> &mut Self
    where
        F: Fn(ListResourcesRequestParams) -> R + Clone + Send + Sync + 'static,
        R: Future + Send,
        R::Output: Into<ListResourcesResult>
    {
        let handler = move |params| {
            let handler = handler.clone();
            async move { handler(params).await.into() }
        };
        self.map_handler("resources/list", handler);
        self
    }

    /// Maps an MCP get prompt request to a specific function
    ///
    /// # Example
    /// ```no_run
    /// use neva::{App, types::Role};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// use neva::types::content;
    /// let mut app = App::new();
    ///
    /// app.map_prompt("analyze-code", |lang: String| async move {
    ///     [
    ///         (format!("Language: {lang}"), Role::User)
    ///     ]
    /// });
    ///
    /// # app.run().await;
    /// # }
    /// ```
    pub fn map_prompt<F, R, Args>(&mut self, name: &str, handler: F) -> &mut Prompt
    where 
        F: PromptHandler<Args, Output = R>,
        R: Into<GetPromptResult> + Send + 'static,
        Args: TryFrom<GetPromptRequestParams, Error = Error> + Send + Sync + 'static,
    {
        self.options.add_prompt(Prompt::new(name, handler))
    }

    /// Connection initialization handler
    async fn init(
        options: Arc<McpOptions>, 
        _params: InitializeRequestParams
    ) -> InitializeResult {
        InitializeResult::new(&options)
    }

    /// Completion request handler
    async fn completion(
        _: Arc<McpOptions>, 
        _params: CompleteRequestParams
    ) -> CompleteResult {
        // TODO: return default as it non-optional capability so far
        CompleteResult::default()
    }
    
    /// Tools request handler
    async fn tools(
        options: Arc<McpOptions>, 
        _params: ListToolsRequestParams
    ) -> ListToolsResult {
        options.tools()
    }

    /// Resources request handler
    async fn resources(
        options: Arc<McpOptions>,
        _params: ListResourcesRequestParams
    ) -> ListResourcesResult {
        options.resources()
    }

    /// Resource templates request handler
    async fn resource_templates(
        options: Arc<McpOptions>, 
        _params: ListResourceTemplatesRequestParams
    ) -> ListResourceTemplatesResult {
        options.resource_templates()
    }
    
    /// Prompts request handler
    async fn prompts(
        options: Arc<McpOptions>, 
        _params: ListPromptsRequestParams
    ) -> ListPromptsResult {
        options.prompts()
    }
    
    /// A tool call request handler
    async fn tool(
        options: Arc<McpOptions>, 
        params: CallToolRequestParams
    ) -> Result<CallToolResponse, Error> {
        match options.get_tool(&params.name) {
            Some(tool) => tool.call(params.into()).await,
            None => Err(Error::new(ErrorCode::InvalidParams, "tool not found")),
        }
    }

    /// A read resource request handler
    async fn resource(
        options: Arc<McpOptions>, 
        params: ReadResourceRequestParams
    ) -> Result<ReadResourceResult, Error> {
        match options.read_resource(&params.uri) {
            Some(Route::Handler(handler)) => handler
                .call(params.into())
                .await,
            _ => Err(Error::new(ErrorCode::ResourceNotFound, "resource not found")),
        }
    }
    
    /// A get prompt request handler
    async fn prompt(
        options: Arc<McpOptions>, 
        params: GetPromptRequestParams
    ) -> Result<GetPromptResult, Error> {
        match options.get_prompt(&params.name) {
            Some(prompt) => prompt.call(params.into()).await,
            None => Err(Error::new(ErrorCode::InvalidParams, "prompt not found")),
        }
    }

    /// Ping request handler
    async fn ping() {}
    
    /// A notifications initialization request handler
    async fn notifications_init() {}
    
    /// A notification cancel request handler
    async fn notifications_cancel() {}
    
    /// A subscription to a resource change request handler
    async fn resource_subscribe(
        _options: Arc<McpOptions>, 
        _params: SubscribeRequestParams
    ) -> Error {
        Error::new(ErrorCode::InvalidRequest, "resource_subscribe not implemented")
    }

    /// An unsubscription to from resource change request handler
    async fn resource_unsubscribe(
        _options: Arc<McpOptions>, 
        _params: UnsubscribeRequestParams
    ) -> Error {
        Error::new(ErrorCode::InvalidRequest, "resource_unsubscribe not implemented")
    }
}

#[cfg(test)]
mod tests {
    
}
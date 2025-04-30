//! Represents an MCP application

use std::collections::HashMap;
use std::future::Future;
use self::{context::{Context, ServerRuntime}, options::{McpOptions, RuntimeMcpOptions}};
use crate::error::{Error, ErrorCode};
use crate::transport::{Receiver, Sender, Transport};
use crate::app::handler::{
    FromHandlerParams,
    GenericHandler,
    HandlerParams,
    RequestFunc,
    RequestHandler
};
use crate::types::{
    InitializeResult, InitializeRequestParams, 
    IntoResponse, Response, Request, Message,
    CompleteResult, CompleteRequestParams, 
    ListToolsRequestParams, CallToolRequestParams, ListToolsResult, CallToolResponse, Tool, ToolHandler, 
    ListResourceTemplatesRequestParams, ListResourceTemplatesResult, ResourceTemplate, 
    ListResourcesRequestParams, ListResourcesResult, ReadResourceRequestParams, ReadResourceResult, 
    SubscribeRequestParams, UnsubscribeRequestParams, Resource, 
    resource::{Route, template::ResourceFunc}, 
    ListPromptsRequestParams, ListPromptsResult, GetPromptRequestParams, GetPromptResult, 
    PromptHandler, Prompt, 
    notification::{Notification, CancelledNotificationParams},
    cursor::Pagination
};
#[cfg(feature = "tracing")]
use crate::types::notification::SetLevelRequestParams;

pub mod options;
pub mod context;
pub(crate) mod handler;

const DEFAULT_PAGE_SIZE: usize = 10;

type RequestHandlers = HashMap<String, RequestHandler<Response>>;

/// Represents an MCP server application
#[derive(Default)]
pub struct App {
    options: McpOptions,
    handlers: RequestHandlers,
}

impl App {
    /// Initializes a new app
    pub fn new() -> Self {
        let mut app = Self { 
            options: McpOptions::default(),
            handlers: HashMap::new()
        };

        app.map_handler(crate::commands::INIT, Self::init);
        app.map_handler(crate::types::completion::commands::COMPLETE, Self::completion);
        
        app.map_handler(crate::types::tool::commands::LIST, Self::tools);
        app.map_handler(crate::types::tool::commands::CALL, Self::tool);
        
        app.map_handler(crate::types::resource::commands::LIST, Self::resources);
        app.map_handler(crate::types::resource::commands::TEMPLATES_LIST, Self::resource_templates);
        app.map_handler(crate::types::resource::commands::READ, Self::resource);
        app.map_handler(crate::types::resource::commands::SUBSCRIBE, Self::resource_subscribe);
        app.map_handler(crate::types::resource::commands::UNSUBSCRIBE, Self::resource_unsubscribe);
        
        app.map_handler(crate::types::prompt::commands::LIST, Self::prompts);
        app.map_handler(crate::types::prompt::commands::GET, Self::prompt);
        
        app.map_handler(crate::types::notification::commands::INITIALIZED, Self::notifications_init);
        app.map_handler(crate::types::notification::commands::CANCELLED, Self::notifications_cancel);
        
        app.map_handler(crate::commands::PING, Self::ping);

        #[cfg(feature = "tracing")]
        app.map_handler(crate::types::notification::commands::SET_LOG_LEVEL, Self::set_log_level);
        
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
        let _ = transport.start();
        
        let (sender, mut receiver) = transport.split();
        let runtime = ServerRuntime::new(sender, self.options, self.handlers);
        
        while let Ok(msg) = receiver.recv().await {
            match msg { 
                Message::Request(req) => Self::handle_request(req, &runtime).await,
                Message::Response(resp) => Self::handle_response(resp, &runtime).await,
                Message::Notification(notification) => Self::handle_notification(notification).await
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
    /// use neva::App;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut app = App::new();
    ///
    /// app.map_resource("res://{name}", "read_resource", |name: String| async move {
    ///     (format!("res://{name}"), format!("Resource: {name} content"))
    /// });
    ///
    /// # app.run().await;
    /// # }
    /// ```
    pub fn map_resource<F, R, Args>(&mut self, uri: &'static str, name: &str, handler: F) -> &mut ResourceTemplate
    where
        F: GenericHandler<Args, Output = R>,
        R: TryInto<ReadResourceResult> + Send + 'static,
        R::Error: Into<Error>,
        Args: TryFrom<ReadResourceRequestParams, Error = Error> + Send + Sync + 'static,
    {
        let handler = ResourceFunc::new(handler);
        let template = ResourceTemplate::new(uri, name);
        
        self.options.add_resource_template(template, handler)
    }

    /// Maps an MCP get prompt request to a specific function
    ///
    /// # Example
    /// ```no_run
    /// use neva::{App, types::Role};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut app = App::new();
    ///
    /// app.map_prompt("analyze-code", |lang: String| async move {
    ///     (format!("Language: {lang}"), Role::User)
    /// });
    ///
    /// # app.run().await;
    /// # }
    /// ```
    pub fn map_prompt<F, R, Args>(&mut self, name: &str, handler: F) -> &mut Prompt
    where
        F: PromptHandler<Args, Output = R>,
        R: TryInto<GetPromptResult> + Send + 'static,
        R::Error: Into<Error>,
        Args: TryFrom<GetPromptRequestParams, Error = Error> + Send + Sync + 'static,
    {
        self.options.add_prompt(Prompt::new(name, handler))
    }

    /// Maps an MCP resource read request to a specific function
    ///
    /// # Example
    /// ```no_run
    /// use neva::{App, types::{Resource, ListResourcesRequestParams}};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
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

    /// Maps a completion request
    ///
    /// # Example
    /// ```no_run
    /// use neva::{App, types::{CompleteRequestParams, CompleteResult}};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut app = App::new();
    ///
    /// app.map_completion(|_params: CompleteRequestParams| async move {
    ///     ["Item 1", "Item 2", "Item 3"]
    /// });
    ///
    /// # app.run().await;
    /// # }
    /// ```
    pub fn map_completion<F, R>(&mut self, handler: F) -> &mut Self
    where
        F: Fn(CompleteRequestParams) -> R + Clone + Send + Sync + 'static,
        R: Future + Send,
        R::Output: Into<CompleteResult>
    {
        let handler = move |params| {
            let handler = handler.clone();
            async move { handler(params).await.into() }
        };
        self.map_handler("completion/complete", handler);
        self
    }

    /// Connection initialization handler
    async fn init(
        options: RuntimeMcpOptions, 
        _params: InitializeRequestParams
    ) -> Result<InitializeResult, Error> {
        Ok(InitializeResult::new(&options))
    }

    /// Completion request handler
    async fn completion() -> CompleteResult {
        // return default as it non-optional capability so far
        CompleteResult::default()
    }
    
    /// Tools request handler
    async fn tools(
        options: RuntimeMcpOptions, 
        params: ListToolsRequestParams
    ) -> ListToolsResult {
        options.tools()
            .paginate(params.cursor, DEFAULT_PAGE_SIZE)
            .into()
    }

    /// Resources request handler
    async fn resources(
        options: RuntimeMcpOptions,
        params: ListResourcesRequestParams
    ) -> ListResourcesResult {
        options.resources()
            .paginate(params.cursor, DEFAULT_PAGE_SIZE)
            .into()
    }

    /// Resource templates request handler
    async fn resource_templates(
        options: RuntimeMcpOptions, 
        params: ListResourceTemplatesRequestParams
    ) -> ListResourceTemplatesResult {
        options.resource_templates()
            .paginate(params.cursor, DEFAULT_PAGE_SIZE)
            .into()
    }
    
    /// Prompts request handler
    async fn prompts(
        options: RuntimeMcpOptions, 
        params: ListPromptsRequestParams
    ) -> ListPromptsResult {
        options.prompts()
            .paginate(params.cursor, DEFAULT_PAGE_SIZE)
            .into()
    }
    
    /// A tool call request handler
    async fn tool(
        ctx: Context,
        options: RuntimeMcpOptions, 
        params: CallToolRequestParams
    ) -> Result<CallToolResponse, Error> {
        match options.get_tool(&params.name) {
            Some(tool) => tool.call(params.with_context(ctx).into()).await,
            None => Err(Error::new(ErrorCode::InvalidParams, "Tool not found"))
        }
    }

    /// A read resource request handler
    async fn resource(
        ctx: Context,
        options: RuntimeMcpOptions, 
        params: ReadResourceRequestParams
    ) -> Result<ReadResourceResult, Error> {
        match options.read_resource(&params.uri) {
            Some(Route::Handler(handler)) => handler.call(params.with_context(ctx).into()).await,
            _ => Err(Error::from(ErrorCode::ResourceNotFound)),
        }
    }
    
    /// A get prompt request handler
    async fn prompt(
        ctx: Context,
        options: RuntimeMcpOptions, 
        params: GetPromptRequestParams
    ) -> Result<GetPromptResult, Error> {
        match options.get_prompt(&params.name) {
            Some(prompt) => prompt.call(params.with_context(ctx).into()).await,
            None => Err(Error::new(ErrorCode::InvalidParams, "Prompt not found"))
        }
    }

    /// Ping request handler
    async fn ping() {}
    
    /// A notification initialization request handler
    async fn notifications_init() {}
    
    /// A notification cancel request handler
    async fn notifications_cancel(
        options: RuntimeMcpOptions,
        params: CancelledNotificationParams
    ) {
        options.cancel_request(&params.request_id).await;
    }
    
    /// A subscription to a resource change request handler
    async fn resource_subscribe(
        _options: RuntimeMcpOptions, 
        _params: SubscribeRequestParams
    ) -> Error {
        Error::new(ErrorCode::InvalidRequest, "resource_subscribe not implemented")
    }

    /// An unsubscription to from resource change request handler
    async fn resource_unsubscribe(
        _options: RuntimeMcpOptions, 
        _params: UnsubscribeRequestParams
    ) -> Error {
        Error::new(ErrorCode::InvalidRequest, "resource_unsubscribe not implemented")
    }
    
    /// Sets the logging level
    #[cfg(feature = "tracing")]
    async fn set_log_level(
        options: RuntimeMcpOptions,
        params: SetLevelRequestParams
    ) -> Result<(), Error> {
        let current_level = options.log_level();
        tracing::debug!(
            logger = "neva", 
            "Logging level has been changed from {:?} to {:?}", current_level, params.level
        );
        
        options.set_log_level(params.level)
    }
    
    async fn handle_request(req: Request, runtime: &ServerRuntime) {
        let req_id = req.id();
        
        let context = runtime.context();
        let options = runtime.options();
        let handlers = runtime.request_handlers();
        let mut sender = runtime.sender();

        let token = options.track_request(&req_id).await;

        tokio::spawn(async move {
            #[cfg(feature = "tracing")]
            tracing::trace!(logger = "neva", "Received: {:?}", req);

            let resp = if let Some(handler) = handlers.get(&req.method) {
                tokio::select! {
                    resp = handler.call(HandlerParams::Request(context, options.clone(), req)) => {
                        options.complete_request(&req_id).await;
                        resp
                    }
                    _ = token.cancelled() => {
                        #[cfg(feature = "tracing")]
                        tracing::debug!(
                            logger = "neva", 
                            "The request with ID: {} has been cancelled", req_id);
                        Err(Error::from(ErrorCode::RequestCancelled))
                    }
                }
            } else {
                Err(Error::from(ErrorCode::MethodNotFound))
            };

            if let Err(_err) = sender.send(resp.into_response(req_id).into()).await {
                #[cfg(feature = "tracing")]
                tracing::error!(
                    logger = "neva", 
                    error = format!("Error sending response: {:?}", _err));
            }
        });
    }
    
    async fn handle_response(resp: Response, runtime: &ServerRuntime) {
        runtime
            .pending_requests()
            .complete(resp)
            .await;
    }
    
    async fn handle_notification(_notification: Notification) {
        #[cfg(feature = "tracing")]
        _notification.write();
    }
}

#[cfg(test)]
mod tests {
    
}
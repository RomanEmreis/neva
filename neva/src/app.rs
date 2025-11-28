//! Represents an MCP application

use tokio_util::sync::CancellationToken;
use self::{context::{Context, ServerRuntime}, options::{McpOptions, RuntimeMcpOptions}};
use crate::error::{Error, ErrorCode};
use crate::transport::{Receiver, Sender, Transport};
use crate::shared;
use crate::middleware::{MwContext, Next, make_fn::make_mw};
use crate::app::handler::{
    FromHandlerParams,
    GenericHandler,
    ListResourcesHandler,
    CompletionHandler,
    HandlerParams,
    RequestFunc,
    RequestHandler
};
use crate::types::{
    InitializeResult, InitializeRequestParams, IntoResponse, Response, Request, RequestId, Message, 
    CompleteResult, ListToolsRequestParams, CallToolRequestParams, ListToolsResult, CallToolResponse, Tool, ToolHandler, 
    ListResourceTemplatesRequestParams, ListResourceTemplatesResult, ResourceTemplate, 
    ListResourcesRequestParams, ListResourcesResult, ReadResourceRequestParams, ReadResourceResult, 
    SubscribeRequestParams, UnsubscribeRequestParams, Resource, resource::template::ResourceFunc, 
    ListPromptsRequestParams, ListPromptsResult, GetPromptRequestParams, GetPromptResult, PromptHandler, Prompt, 
    notification::{Notification, CancelledNotificationParams}, 
    cursor::Pagination, Uri
};
use std::{
    fmt::{Debug, Formatter},
    collections::HashMap,
};

#[cfg(feature = "tracing")]
use {
    crate::types::notification::SetLevelRequestParams,
    tracing::Instrument
};
#[cfg(feature = "di")]
use volga_di::{Container, ContainerBuilder};

pub mod options;
pub mod context;
pub(crate) mod handler;
mod collection;

const DEFAULT_PAGE_SIZE: usize = 10;

type RequestHandlers = HashMap<String, RequestHandler<Response>>;

/// Represents an MCP server application
#[derive(Default)]
pub struct App {
    /// MCP server options
    pub(super) options: McpOptions,
    
    /// DI container
    #[cfg(feature = "di")]
    pub(super) container: ContainerBuilder,
    
    /// MCP server request handlers
    handlers: RequestHandlers,
}

impl Debug for App {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("App { ... }")
    }
}

impl App {
    /// Initializes a new MCP app
    pub fn new() -> Self {
        let mut app = Self { 
            options: McpOptions::default(),
            handlers: HashMap::new(),
            #[cfg(feature = "di")]
            container: ContainerBuilder::new(),
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
        #[cfg(feature = "macros")]
        self.register_methods();
        
        #[cfg(feature = "tracing")]
        self.options.add_middleware(make_mw(Self::tracing_middleware));
        self.options.add_middleware(make_mw(Self::message_middleware));
        
        let mut transport = self.options.transport();
        let cancellation_token = transport.start();
        self.wait_for_shutdown_signal(cancellation_token.clone());
        
        let (sender, mut receiver) = transport.split();
        let runtime = ServerRuntime::new(
            sender, 
            self.options, 
            self.handlers,
            #[cfg(feature = "di")]
            self.container.build()
        );
        loop {
            tokio::select! {
                biased;
                _ = cancellation_token.cancelled() => break,
                msg = receiver.recv() => {
                    match msg { 
                        Ok(msg) => {
                            tokio::spawn(Self::execute(msg, runtime.clone()));
                        },
                        Err(_err) => {
                            #[cfg(feature = "tracing")]
                            tracing::error!("Error handling message: {:?}", _err);
                            break;
                        }
                    }
                }
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
    pub fn map_handler<F, R, Args>(&mut self, name: impl Into<String>, handler: F) -> &mut Self
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
    pub fn map_tool<F, R, Args>(&mut self, name: impl Into<String>, handler: F) -> &mut Tool
    where
        F: ToolHandler<Args, Output = R>,
        R: Into<CallToolResponse> + Send + 'static,
        Args: TryFrom<CallToolRequestParams, Error = Error> + Send + Sync + 'static,
    {
        self.options.add_tool(Tool::new(name, handler))
    }
    
    /// Adds a known resource
    pub fn add_resource<U: Into<Uri>, S: Into<String>>(&mut self, uri: U, name: S) -> &mut Resource {
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
    pub fn map_resource<F, R, Args>(
        &mut self, 
        uri: impl Into<Uri>, 
        name: impl Into<String>, 
        handler: F
    ) -> &mut ResourceTemplate
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

    /// Maps an MCP get a prompt request to a specific function
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
    pub fn map_prompt<F, R, Args>(&mut self, name: impl Into<String>, handler: F) -> &mut Prompt
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
    pub fn map_resources<F, Args, R>(&mut self, handler: F) -> &mut Self
    where
        F: ListResourcesHandler<Args, Output = R> + Clone + Send + Sync + 'static,
        Args: FromHandlerParams + Send + Sync + 'static,
        R: Into<ListResourcesResult>
    {
        let handler = move |params, args| {
            let handler = handler.clone();
            async move { handler.call(params, args).await.into() }
        };
        self.map_handler(crate::types::resource::commands::LIST, handler);
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
    pub fn map_completion<F, Args, R>(&mut self, handler: F) -> &mut Self
    where
        F: CompletionHandler<Args, Output = R> + Clone + Send + Sync + 'static,
        Args: FromHandlerParams + Send + Sync + 'static,
        R: Into<CompleteResult>
    {
        let handler = move |params, args| {
            let handler = handler.clone();
            async move { handler.call(params, args).await.into() }
        };
        self.map_handler(crate::types::completion::commands::COMPLETE, handler);
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
        // return default as its non-optional capability so far
        CompleteResult::default()
    }
    
    /// Tools request handler
    async fn tools(
        options: RuntimeMcpOptions, 
        params: ListToolsRequestParams
    ) -> ListToolsResult {
        options.list_tools()
            .await
            .paginate(params.cursor, DEFAULT_PAGE_SIZE)
            .into()
    }

    /// Resources request handler
    async fn resources(
        options: RuntimeMcpOptions,
        params: ListResourcesRequestParams
    ) -> ListResourcesResult {
        options.list_resources()
            .await
            .paginate(params.cursor, DEFAULT_PAGE_SIZE)
            .into()
    }

    /// Resource templates request handler
    async fn resource_templates(
        options: RuntimeMcpOptions, 
        params: ListResourceTemplatesRequestParams
    ) -> ListResourceTemplatesResult {
        options.list_resource_templates()
            .await
            .paginate(params.cursor, DEFAULT_PAGE_SIZE)
            .into()
    }
    
    /// Prompts request handler
    async fn prompts(
        options: RuntimeMcpOptions, 
        params: ListPromptsRequestParams
    ) -> ListPromptsResult {
        options.list_prompts()
            .await
            .paginate(params.cursor, DEFAULT_PAGE_SIZE)
            .into()
    }
    
    /// A tool call request handler
    async fn tool(ctx: Context, params: CallToolRequestParams) -> Result<CallToolResponse, Error> {
        ctx.call_tool(params).await
    }

    /// A read resource request handler
    async fn resource(ctx: Context, params: ReadResourceRequestParams) -> Result<ReadResourceResult, Error> {
        ctx.read_resource(params).await
    }
    
    /// A get prompt request handler
    async fn prompt(ctx: Context, params: GetPromptRequestParams) -> Result<GetPromptResult, Error> {
        ctx.get_prompt(params).await
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
        options.cancel_request(&params.request_id);
    }
    
    /// A subscription to a resource change request handler
    async fn resource_subscribe(
        mut ctx: Context, 
        params: SubscribeRequestParams
    ) {
        ctx.subscribe_to_resource(params.uri);
    }

    /// An unsubscription to from resource change request handler
    async fn resource_unsubscribe(
        mut ctx: Context,
        params: UnsubscribeRequestParams
    ) {
        ctx.unsubscribe_from_resource(&params.uri);
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
    
    #[cfg(feature = "tracing")]
    async fn tracing_middleware(ctx: MwContext, next: Next) -> Response {
        let span = create_tracing_span(ctx.session_id().cloned());
        next(ctx)
            .instrument(span)
            .await
    }

    #[inline]
    async fn execute(msg: Message, runtime: ServerRuntime) {
        runtime.execute(msg).await;
    }

    async fn message_middleware(ctx: MwContext, _: Next) -> Response {
        let MwContext { 
            msg, 
            runtime,
            #[cfg(feature = "di")]
            scope
        } = ctx;
        let id = msg.id();
        let mut sender = runtime.sender();
        
        let resp = Self::handle_message(
            msg, 
            runtime,
            #[cfg(feature = "di")]
            scope
        ).await;

        if let Err(_err) = sender.send(resp.into()).await {
            #[cfg(feature = "tracing")]
            tracing::error!(
                logger = "neva", 
                error = format!("Error sending response: {:?}", _err));
        }
        
        Response::empty(id)
    }
    
    #[inline]
    async fn handle_message(
        msg: Message, 
        runtime: ServerRuntime,
        #[cfg(feature = "di")]
        scope: Container
    ) -> Response {
        match msg {
            Message::Request(req) => Self::handle_request(
                req, 
                runtime, 
                #[cfg(feature = "di")] 
                scope
            ).await,
            Message::Response(resp) => Self::handle_response(resp, runtime).await,
            Message::Notification(notification) => Self::handle_notification(notification).await
        }
    }
    
    async fn handle_request(
        req: Request, 
        runtime: ServerRuntime,
        #[cfg(feature = "di")]
        scope: Container
    ) -> Response {
        #[cfg(feature = "http-server")]
        let mut req = req;
        let req_id = req.id();
        let session_id = req.session_id;
        let full_id = req.full_id();

        #[cfg(not(feature = "http-server"))]
        let context = runtime.context(session_id);
        
        #[cfg(feature = "http-server")]
        let context = {
            let headers = std::mem::take(&mut req.headers);
            let claims = req.claims
                .take()
                .map(|c| *c);
            runtime.context(session_id, headers, claims)
        };
        
        #[cfg(feature = "di")]
        let context = context.with_scope(scope);
        
        let options = runtime.options();
        let handlers = runtime.request_handlers();
        let token = options.track_request(&full_id);

        #[cfg(feature = "tracing")]
        tracing::trace!(logger = "neva", "Received: {:?}", req);
        let resp = if let Some(handler) = handlers.get(&req.method) {
            tokio::select! {
                resp = handler.call(HandlerParams::Request(context, req)) => {
                    options.complete_request(&full_id);
                    resp
                }
                _ = token.cancelled() => {
                    #[cfg(feature = "tracing")]
                    tracing::debug!(
                        logger = "neva", 
                        "The request with ID: {} has been cancelled", full_id);
                        Err(Error::from(ErrorCode::RequestCancelled))
                    }
                }
        } else {
            Err(Error::from(ErrorCode::MethodNotFound))
        };

        let mut resp = resp.into_response(req_id);
        if let Some(session_id) = session_id {
            resp = resp.set_session_id(session_id);
        }
        resp
    }
    
    async fn handle_response(resp: Response, runtime: ServerRuntime) -> Response {
        let resp_id = resp.id().clone();
        let session_id = resp
            .session_id()
            .cloned();
        
        runtime
            .pending_requests()
            .complete(resp);

        let mut resp = Response::empty(resp_id);
        if let Some(session_id) = session_id {
            resp = resp.set_session_id(session_id);
        }
        resp
    }
    
    #[inline]
    async fn handle_notification(notification: Notification) -> Response {
        if let crate::types::notification::commands::MESSAGE = notification.method.as_str() {
            #[cfg(feature = "tracing")]
            notification.write();
        }
        Response::empty(RequestId::default())
    }

    #[inline]
    fn wait_for_shutdown_signal(&mut self, token: CancellationToken) {
        shared::wait_for_shutdown_signal(token);
    }
}

#[cfg(feature = "tracing")]
fn create_tracing_span(session_id: Option<uuid::Uuid>) -> tracing::Span {
    if let Some(mcp_session_id) = session_id {
        tracing::info_span!("request", mcp_session_id = mcp_session_id.to_string())
    } else {
        tracing::info_span!("request")
    }
}

#[cfg(test)]
mod tests {
    
}
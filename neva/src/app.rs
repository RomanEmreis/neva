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
    MessageEnvelope, MessageBatch,
    CompleteResult, ListToolsRequestParams, CallToolRequestParams, ListToolsResult, CallToolResponse, Tool, ToolHandler,
    ListResourceTemplatesRequestParams, ListResourceTemplatesResult, ResourceTemplate,
    ListResourcesRequestParams, ListResourcesResult, ReadResourceRequestParams, ReadResourceResult,
    SubscribeRequestParams, UnsubscribeRequestParams, Resource, resource::template::ResourceFunc,
    ListPromptsRequestParams, ListPromptsResult, GetPromptRequestParams, GetPromptResult, PromptHandler, Prompt,
    notification::{Notification, CancelledNotificationParams},
    cursor::Pagination, Uri
};

#[cfg(feature = "tasks")]
use crate::types::{
    ListTasksRequestParams, ListTasksResult, CancelTaskRequestParams,
    GetTaskRequestParams, GetTaskPayloadRequestParams, Task, TaskPayload,
};
#[cfg(feature = "tasks")]
use context::ToolOrTaskResponse;

use std::{
    fmt::{Debug, Formatter},
    collections::HashMap,
    sync::Arc,
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

        #[cfg(feature = "tasks")]
        {
            app.map_handler(crate::types::task::commands::LIST, Self::tasks);
            app.map_handler(crate::types::task::commands::GET, Self::task);
            app.map_handler(crate::types::task::commands::CANCEL, Self::cancel_task);
            app.map_handler(crate::types::task::commands::RESULT, Self::task_result);
        }
        
        app.map_handler(crate::commands::PING, Self::ping);

        #[cfg(feature = "tracing")]
        app.map_handler(crate::types::notification::commands::SET_LOG_LEVEL, Self::set_log_level);
        
        app
    }

    /// Starts the [`App`] with its own Tokio runtime.
    ///
    /// This method is intended for simple use cases where you don't already have a Tokio runtime setup.
    /// Internally, it creates and runs a multi-threaded Tokio runtime to execute the application.
    ///
    /// **Note:** This method **must not** be called from within an existing Tokio runtime
    /// (e.g., inside an `#[tokio::main]` async function), or it will panic.
    /// If you are already using Tokio in your application, use [`App::run`] instead.
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    ///
    /// // configure tools, resources, prompts
    ///
    /// app.run_blocking()
    /// # }
    /// ```
    pub fn run_blocking(self) {
        if tokio::runtime::Handle::try_current().is_ok() {
            panic!("`App::run_blocking()` cannot be called inside an existing Tokio runtime. Use `run().await` instead.");
        }

        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(err) => {
                #[cfg(feature = "tracing")]
                tracing::error!("failed to start the runtime: {err:#}");
                #[cfg(not(feature = "tracing"))]
                eprintln!("failed to start the runtime: {err:#}");
                return;
            }
        };

        runtime.block_on(async {
            self.run().await
        });
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
                        Ok(msg) => match msg {
                            Message::Batch(batch) => {
                                tokio::spawn(Self::execute_batch(batch, runtime.clone()));
                            },
                            msg => {
                                tokio::spawn(Self::execute(msg, runtime.clone()));
                            }
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
    #[cfg(not(feature = "tasks"))]
    async fn tool(ctx: Context, params: CallToolRequestParams) -> Result<CallToolResponse, Error> {
        ctx.call_tool(params).await
    }

    /// A tool call request handler
    #[cfg(feature = "tasks")]
    async fn tool(ctx: Context, params: CallToolRequestParams) -> Result<ToolOrTaskResponse, Error> {
        ctx.call_tool_with_task(params).await
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
    
    /// Tasks request handler
    #[cfg(feature = "tasks")]
    async fn tasks(
        options: RuntimeMcpOptions,
        params: ListTasksRequestParams
    ) -> Result<ListTasksResult, Error> {
        if !options.is_tasks_list_supported() { 
            return Err(Error::new(
                ErrorCode::InvalidRequest, 
                "Server does not support support tasks/list requests."));
        }
        Ok(options
            .list_tasks()
            .paginate(params.cursor, DEFAULT_PAGE_SIZE)
            .into())
    }

    /// A cancel task request handler
    #[cfg(feature = "tasks")]
    async fn cancel_task(
        options: RuntimeMcpOptions,
        params: CancelTaskRequestParams
    ) -> Result<Task, Error> {
        if options.is_tasks_cancellation_supported() {
            options.cancel_task(&params.id)
        } else {
            Err(Error::new(
                ErrorCode::InvalidRequest,
                "Server does not support support tasks/cancel requests."))
        }
    }

    /// A task status retrieval request handler
    #[cfg(feature = "tasks")]
    async fn task(
        options: RuntimeMcpOptions,
        params: GetTaskRequestParams
    ) -> Result<Task, Error> {
        options.get_task_status(&params.id)
    }

    /// A task result retrieval request handler
    #[cfg(feature = "tasks")]
    async fn task_result(
        options: RuntimeMcpOptions,
        params: GetTaskPayloadRequestParams
    ) -> Result<TaskPayload, Error> {
        options.get_task_result(&params.id).await
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

    async fn execute_batch(batch: MessageBatch, runtime: ServerRuntime) {
        use futures_util::future::join_all;
        use crate::transport::TransportProtoSender;

        // Capture the incoming batch's correlation and HTTP-context fields.
        // `id` + `session_id` are needed so the response batch can be routed
        // back to the correct waiting HTTP handler.  `headers` and `claims`
        // are copied onto every inner Request so that middleware (auth checks,
        // role/permission guards, SSE routing) sees the original HTTP context.
        let batch_id = batch.id.clone();
        let batch_session_id = batch.session_id;
        #[cfg(feature = "http-server")]
        let batch_headers = batch.headers.clone();
        #[cfg(feature = "http-server")]
        let batch_claims = batch.claims.clone();

        let real_sender = runtime.sender();

        // Collect responses produced by batch request handlers in-memory.
        // Server-initiated messages (sampling, elicitation, notifications) go
        // straight to the real transport inside BatchCollect::send, so handlers
        // that call ctx.elicit()/ctx.sample() never deadlock.
        //
        // Crucially, background tasks that capture a BatchCollect sender clone
        // do NOT block the batch response: we only wait for the join_all futures,
        // then snapshot whatever responses have been collected so far.
        let responses: Arc<std::sync::Mutex<Vec<MessageEnvelope>>> = Arc::default();
        let batch_sender = TransportProtoSender::BatchCollect {
            real_sender: Arc::new(tokio::sync::Mutex::new(real_sender.clone())),
            responses: Arc::clone(&responses),
        };

        let futures = batch.into_iter().map(|envelope| {
            let runtime = runtime.clone();
            let sender = batch_sender.clone();
            // Clone per-iteration so each async move block owns its own copy.
            #[cfg(feature = "http-server")]
            let batch_headers = batch_headers.clone();
            #[cfg(feature = "http-server")]
            let batch_claims = batch_claims.clone();
            async move {
                match envelope {
                    MessageEnvelope::Request(mut req) => {
                        // Copy the batch's HTTP metadata onto the inner request
                        // so that session/auth context is preserved: without
                        // this, role/permission checks can fail with a valid
                        // token and server-initiated follow-up calls (sampling,
                        // elicitation) cannot be routed back over SSE.
                        req.session_id = batch_session_id;
                        #[cfg(feature = "http-server")]
                        {
                            req.headers = batch_headers;
                            req.claims = batch_claims;
                        }
                        // Route through the full middleware chain with the
                        // batch-collect sender so registered middlewares apply.
                        runtime
                            .with_sender(sender)
                            .execute(Message::Request(req))
                            .await;
                    }
                    MessageEnvelope::Notification(notification) => {
                        Self::handle_notification(notification).await;
                    }
                    MessageEnvelope::Response(mut resp) => {
                        // Apply the batch's session context so that
                        // `resp.full_id()` (= session_id + resp_id) matches
                        // the key used when the server registered the pending
                        // request via `send_request`. Without this the lookup
                        // in `RequestQueue::complete` misses and the pending
                        // handler leaks.
                        if let Some(session_id) = batch_session_id {
                            resp = resp.set_session_id(session_id);
                        }
                        #[cfg(feature = "http-server")]
                        {
                            resp = resp.set_headers(batch_headers);
                        }
                        Self::handle_response(resp, runtime).await;
                    }
                }
            }
        });

        join_all(futures).await;

        // Snapshot collected responses. Any response that a background task
        // produces after this point is silently discarded — it arrived too
        // late to be included in the batch reply.
        let envelopes = responses
            .lock()
            .map(|mut guard| std::mem::take(&mut *guard))
            .unwrap_or_default();

        if envelopes.is_empty() {
            return; // all items were notifications/responses - no reply needed
        }

        let mut resp_batch = match MessageBatch::new(envelopes) {
            Ok(b) => b,
            Err(_err) => {
                // Unreachable in practice: envelopes are non-empty above.
                #[cfg(feature = "tracing")]
                tracing::error!(logger = "neva", "Failed to construct batch response: {:?}", _err);
                return;
            }
        };
        // Restore the correlation id+session so the HTTP transport can match
        // this response batch to the waiting HTTP handler.
        resp_batch.id = batch_id;
        resp_batch.session_id = batch_session_id;

        let mut sender = real_sender;
        if let Err(_err) = sender.send(Message::Batch(resp_batch)).await {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "Error sending batch response: {:?}", _err);
        }
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
            Message::Notification(notification) => Self::handle_notification(notification).await,
            Message::Batch(_) => {
                // Batches are dispatched via execute_batch before reaching handle_message
                unreachable!("Message::Batch should be intercepted in App::run before handle_message")
            }
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
    use crate::types::{MessageBatch, MessageEnvelope};

    #[test]
    fn batch_filtering_notifications_yield_no_response_slots() {
        use crate::types::notification::Notification;

        // Build a notification-only batch
        let batch = MessageBatch::new(vec![
            MessageEnvelope::Notification(Notification::new("notifications/foo", None)),
            MessageEnvelope::Notification(Notification::new("notifications/bar", None)),
        ]).expect("non-empty batch must be constructable");

        // Replicate the filter logic from execute_batch:
        // Request → Some(response slot), Notification/Response → None
        let response_slots: Vec<MessageEnvelope> = batch
            .into_iter()
            .filter_map(|envelope| match envelope {
                MessageEnvelope::Request(_) => Some(envelope),
                _ => None,
            })
            .collect();

        assert!(
            response_slots.is_empty(),
            "notification-only batch must produce zero response slots"
        );
    }

    #[test]
    fn batch_filtering_requests_yield_response_slots() {
        use crate::types::{Request, RequestId};

        // Build a request-only batch
        let req1 = Request::new(Some(RequestId::Number(1)), "tools/list", None::<()>);
        let req2 = Request::new(Some(RequestId::Number(2)), "ping", None::<()>);
        let batch = MessageBatch::new(vec![
            MessageEnvelope::Request(req1),
            MessageEnvelope::Request(req2),
        ]).expect("non-empty batch must be constructable");

        // Replicate the filter: only Request envelopes produce response slots
        let response_slots: Vec<MessageEnvelope> = batch
            .into_iter()
            .filter_map(|envelope| match envelope {
                MessageEnvelope::Request(_) => Some(envelope),
                _ => None,
            })
            .collect();

        assert_eq!(response_slots.len(), 2, "two requests must produce two response slots");
    }
}
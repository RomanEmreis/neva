//! Server runtime context utilities

use tokio::time::timeout;
use crate::error::{Error, ErrorCode};
use crate::transport::Sender;
use super::{options::{McpOptions, RuntimeMcpOptions}, handler::RequestHandler};
use crate::{
    shared::{IntoArgs, RequestQueue}, 
    middleware::{MwContext, Next}, 
    transport::TransportProtoSender, 
    types::{
        Tool, CallToolRequestParams, CallToolResponse,
        ToolUse, ToolResult,
        Resource, ReadResourceRequestParams, ReadResourceResult,
        Prompt, GetPromptRequestParams, GetPromptResult,
        RequestId, Request, Response, Uri,
        Message,
        notification::Notification,
        root::{ListRootsRequestParams, ListRootsResult},
        resource::SubscribeRequestParams,
        sampling::{CreateMessageRequestParams, CreateMessageResult},
        elicitation::{ElicitRequestParams, ElicitResult, ElicitationCompleteParams}
    }
};
use std::{
    fmt::{Debug, Formatter},
    collections::HashMap,
    time::Duration,
    sync::Arc
};

#[cfg(feature = "http-server")]
use {
    crate::transport::http::server::{validate_roles, validate_permissions},
    crate::auth::DefaultClaims,
    volga::headers::HeaderMap
};
#[cfg(feature = "di")]
use volga_di::Container;
#[cfg(feature = "tasks")]
use serde::de::DeserializeOwned;
#[cfg(feature = "tasks")]
use crate::{
    shared::Either,
    types::{
        Task, TaskPayload, CreateTaskResult, tool::TaskSupport,
        ListTasksRequestParams,ListTasksResult, Cursor,
        CancelTaskRequestParams, GetTaskPayloadRequestParams, GetTaskRequestParams,
    },
};

#[cfg(feature = "tasks")]
pub(crate) type ToolOrTaskResponse = Either<CreateTaskResult, CallToolResponse>;

type RequestHandlers = HashMap<String, RequestHandler<Response>>;

/// Represents a Server runtime
#[derive(Clone)]
pub(crate) struct ServerRuntime {
    /// Represents MCP server options
    options: RuntimeMcpOptions,
    
    /// Represents registered request handlers
    handlers: Arc<RequestHandlers>,
    
    /// Represents a queue of pending requests
    pending: RequestQueue,
    
    /// Represents a sender that depends on selected transport protocol
    sender: TransportProtoSender,
    
    /// Global middlewares entrypoint
    mw_start: Option<Next>,
    
    /// Represents a DI container
    #[cfg(feature = "di")]
    pub(crate) container: Container,
}

/// Represents MCP Request Context
#[derive(Clone)]
pub struct Context {
    /// Represents current session id
    pub session_id: Option<uuid::Uuid>,
    
    /// Represents HTTP headers of the current request
    #[cfg(feature = "http-server")]
    pub headers: HeaderMap,
    
    /// Represents JWT claims of the current request
    #[cfg(feature = "http-server")]
    pub(crate) claims: Option<DefaultClaims>,
    
    /// Represents MCP server options
    pub(crate) options: RuntimeMcpOptions,
    
    /// Represents a queue of pending requests
    pending: RequestQueue,
    
    /// Represents a sender that depends on selected transport protocol
    sender: TransportProtoSender,
    
    /// Represents a timeout for the current request
    timeout: Duration,

    /// Represents a DI scope
    #[cfg(feature = "di")]
    pub(crate) scope: Option<Container>,
}

impl Debug for Context {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context")
            .field("session_id", &self.session_id)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl ServerRuntime {
    /// Creates a new server runtime
    pub(crate) fn new(
        sender: TransportProtoSender, 
        mut options: McpOptions,
        handlers: RequestHandlers,
        #[cfg(feature = "di")]
        container: Container
    ) -> Self {
        let middlewares = options.middlewares.take();
        Self {
            pending: Default::default(),
            handlers: Arc::new(handlers),
            options: options.into_runtime(),
            mw_start: middlewares.and_then(|mw| mw.compose()),
            sender,
            #[cfg(feature = "di")]
            container,
        }
    }
    
    /// Provides a [`RuntimeMcpOptions`]
    pub(crate) fn options(&self) ->  RuntimeMcpOptions {
        self.options.clone()
    }

    /// Provides the current connections sender
    pub(crate) fn sender(&self) ->  TransportProtoSender {
        self.sender.clone()
    }
    
    /// Provides a hash map of registered request handlers
    pub(crate) fn request_handlers(&self) ->  Arc<RequestHandlers> {
        self.handlers.clone()
    }
    
    /// Creates a new MCP request [`Context`]
    #[cfg(not(feature = "http-server"))]
    pub(crate) fn context(&self, session_id: Option<uuid::Uuid>) -> Context {
        Context {
            session_id,
            pending: self.pending.clone(),
            sender: self.sender.clone(),
            options: self.options.clone(),
            timeout: self.options.request_timeout,
            #[cfg(feature = "di")]
            scope: None,
        }
    }

    /// Creates a new MCP request [`Context`]
    #[cfg(feature = "http-server")]
    pub(crate) fn context(
        &self, 
        session_id: Option<uuid::Uuid>, 
        headers: HeaderMap, 
        claims: Option<DefaultClaims>
    ) -> Context {
        Context {
            session_id,
            headers,
            claims,
            pending: self.pending.clone(),
            sender: self.sender.clone(),
            options: self.options.clone(),
            timeout: self.options.request_timeout,
            #[cfg(feature = "di")]
            scope: None,
        }
    }
    
    /// Provides a "queue" of pending requests
    pub(crate) fn pending_requests(&self) -> &RequestQueue {
        &self.pending
    }
    
    /// Starts the middleware pipeline
    #[inline]
    pub(crate) async fn execute(self, msg: Message) {
        if let Some(mw_start) = self.mw_start.clone() {
            mw_start(MwContext::msg(msg, self)).await;
        }
    }
}

impl Context {
    /// Returns a list of all available tools
    pub async fn tools(&self) -> Vec<Tool> {
        self.options.tools.values().await
    }
    
    /// Finds a tool by `name`
    pub async fn find_tool(&self, name: &str) -> Option<Tool> {
        self.options.tools.get(name).await
    }

    /// Returns a list of tools by name.
    /// If some tools requested in `names` are missing, they won't be in the result list.
    pub async fn find_tools(&self, names: impl IntoIterator<Item = &str>) -> Vec<Tool> {
        futures_util::future::join_all(
            names.into_iter()
                .map(|name| self.options.tools.get(name)))
            .await
            .into_iter()
            .flatten()
            .collect()
    }
    
    /// Initiates a tool call once a [`ToolUse`] request received from assistant 
    /// withing a sampling window.
    ///
    /// For multiple [`ToolUse`] requests, use the [`Context::use_tools`] method.
    /// 
    /// # Example
    /// ```no_run
    /// # #[cfg(feature = "server-macros")] {
    /// use neva::prelude::*;
    ///
    /// #[tool]
    /// async fn analyze_weather(ctx: Context, city: String) -> Result<(), Error> {
    ///     let args = ("city", city);
    ///     let weather = ctx.use_tool(ToolUse::new("get_weather", args)).await;
    ///     
    ///     // do something with the weather result
    ///
    /// # Ok(())
    /// }
    /// 
    /// #[tool]
    /// async fn get_weather(city: String) -> String {
    ///     // ...
    /// 
    ///     format!("Sunny in {city}")
    /// }
    /// # }
    /// ```
    pub async fn use_tool(&self, tool: ToolUse) -> ToolResult {
        let id = tool.id.clone();
        let res = self.clone()
            .call_tool(tool.into())
            .await;
        match res {
            Ok(res) => ToolResult::new(id, res),
            Err(err) => ToolResult::error(id, err)
        }
    }
    
    /// Initiates a parallel tool calls for multiple [`ToolUse`] requests.
    ///
    /// For a single [`ToolUse`] use the [`Context::use_tool`] method.
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(feature = "server-macros")] {
    /// use neva::prelude::*;
    ///
    /// #[tool]
    /// async fn analyze_weather(ctx: Context) -> Result<(), Error> {
    ///     let weather = ctx.use_tools([
    ///         ToolUse::new("get_weather", ("city", "London")),
    ///         ToolUse::new("get_weather", ("city", "Paris"))
    ///     ]).await;
    ///     
    ///     // do something with the weather result
    ///
    /// # Ok(())
    /// }
    /// # }
    /// ```
    pub async fn use_tools<I>(&self, tools: I) -> Vec<ToolResult>
    where 
        I : IntoIterator<Item = ToolUse>
    {
        futures_util::future::join_all(
            tools.into_iter().map(|t| self.use_tool(t)))
            .await
    }
    
    /// Gets the prompt by name
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(feature = "server-macros")] {
    /// use neva::prelude::*;
    ///
    /// #[tool]
    /// async fn analyze_weather(ctx: Context, city: String) -> Result<(), Error> {
    ///     let prompt = ctx.prompt("get_weather", ("city", city)).await?;
    ///     
    ///     // do something with the prompt
    ///
    /// # Ok(())
    /// }
    ///
    /// #[prompt]
    /// async fn get_weather(city: String) -> PromptMessage {
    ///     PromptMessage::user()
    ///         .with(format!("What's the weather in {city}"))
    /// }
    /// # }
    /// ```
    pub async fn prompt<N, Args>(
        &self, 
        name: N, 
        args: Args
    ) -> Result<GetPromptResult, Error>
    where
        N: Into<String>,
        Args: IntoArgs,
    {
        let params = GetPromptRequestParams {
            name: name.into(),
            args: args.into_args(),
            meta: None
        };
        self.clone()
            .get_prompt(params)
            .await
    }
    
    /// Reads a resource contents
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(feature = "server-macros")] {
    /// use neva::prelude::*;
    ///
    /// #[tool]
    /// async fn summarize_document(ctx: Context, doc_uri: Uri) -> Result<(), Error> {
    ///     let doc = ctx.resource(doc_uri).await?;
    ///     
    ///     // do something with the doc
    ///
    /// # Ok(())
    /// }
    /// 
    /// #[resource(uri = "file://{name}")]
    /// async fn get_doc(name: String) -> TextResourceContents {
    ///     // read the doc
    /// 
    /// # TextResourceContents::new("", "") 
    /// }
    /// # }
    /// ```
    pub async fn resource(&self, uri: impl Into<Uri>) -> Result<ReadResourceResult, Error> {
        let uri = uri.into();
        let params = ReadResourceRequestParams::from(uri);
        self.clone()
            .read_resource(params)
            .await
    }
    
    /// Adds a new resource and notifies clients
    pub async fn add_resource(&mut self, res: impl Into<Resource>) -> Result<(), Error> {
        let res: Resource = res.into();
        self.options
            .resources
            .insert(res.name.clone(), res)
            .await?;

        if self.options.is_resource_list_changed_supported() {
            self.send_notification(
                crate::types::resource::commands::LIST_CHANGED,
                None
            ).await 
        } else { 
            Ok(())
        }
    }

    /// Removes a resource and notifies clients
    pub async fn remove_resource(&mut self, uri: impl Into<Uri>) -> Result<Option<Resource>, Error> {
        let removed = self.options
            .resources
            .remove(&uri.into())
            .await?;

        if removed.is_some() && self.options.is_resource_list_changed_supported() {
            self.send_notification(
                crate::types::resource::commands::LIST_CHANGED,
                None
            ).await?;   
        }
        
        Ok(removed)
    }
    
    /// Sends a [`Notification`] that the resource with the `uri` has been updated
    pub async fn resource_updated(&mut self, uri: impl Into<Uri>) -> Result<(), Error> {
        if !self.options.is_resource_subscription_supported() { 
            return Err(Error::new(
                ErrorCode::MethodNotFound, 
                "Server does not support sending resource/updated notifications"))
        }
        
        let uri = uri.into();
        if self.is_subscribed(&uri) {
            let params = serde_json::to_value(SubscribeRequestParams::from(uri)).ok();
            self.send_notification(crate::types::resource::commands::UPDATED, params).await   
        } else { 
            Ok(())
        }
    }

    /// Adds a subscription to the resource with the [`Uri`]
    pub fn subscribe_to_resource(&mut self, uri: impl Into<Uri>) {
        self.options
            .resource_subscriptions
            .insert(uri.into());
    }
    
    /// Removes a subscription to the resource with the [`Uri`]
    pub fn unsubscribe_from_resource(&mut self, uri: &Uri) {
        self.options
            .resource_subscriptions
            .remove(uri);
    }
    
    /// Returns `true` if there is a subscription to changes of the resource with the [`Uri`]
    pub fn is_subscribed(&self, uri: &Uri) -> bool {
        self.options
            .resource_subscriptions
            .contains(uri)
    }

    /// Adds a new prompt and notifies clients
    pub async fn add_prompt(&mut self, prompt: Prompt) -> Result<(), Error> {
        self.options
            .prompts
            .insert(prompt.name.clone(), prompt)
            .await?;

        if self.options.is_prompts_list_changed_supported() {
            self.send_notification(
                crate::types::prompt::commands::LIST_CHANGED,
                None
            ).await
        } else {
            Ok(())
        }
    }

    /// Removes a prompt and notifies clients
    pub async fn remove_prompt(&mut self, name: impl Into<String>) -> Result<Option<Prompt>, Error> {
        let removed = self.options
            .prompts
            .remove(&name.into())
            .await?;

        if removed.is_some() && self.options.is_prompts_list_changed_supported() {
            self.send_notification(
                crate::types::prompt::commands::LIST_CHANGED,
                None
            ).await?;
        }

        Ok(removed)
    }

    /// Adds a new prompt and notifies clients
    pub async fn add_tool(&mut self, tool: Tool) -> Result<(), Error> {
        self.options
            .tools
            .insert(tool.name.clone(), tool)
            .await?;

        if self.options.is_tools_list_changed_supported() {
            self.send_notification(
                crate::types::tool::commands::LIST_CHANGED,
                None
            ).await
        } else {
            Ok(())
        }
    }

    /// Removes a tool and notifies clients
    pub async fn remove_tool(&mut self, name: impl Into<String>) -> Result<Option<Tool>, Error> {
        let removed = self.options
            .tools
            .remove(&name.into())
            .await?;

        if removed.is_some() && self.options.is_tools_list_changed_supported() {
            self.send_notification(
                crate::types::tool::commands::LIST_CHANGED,
                None
            ).await?;
        }

        Ok(removed)
    }
    
    #[inline]
    pub(crate) async fn read_resource(self, params: ReadResourceRequestParams) -> Result<ReadResourceResult, Error> {
        let opt = self.options.clone();
        match opt.read_resource(&params.uri) {
            Some((handler, args)) => {
                #[cfg(feature = "http-server")]
                {
                    let template = opt.resources_templates
                        .get(&handler.template)
                        .await;
                    self.validate_claims(
                        template.as_ref().and_then(|t| t.roles.as_deref()),
                        template.as_ref().and_then(|t| t.permissions.as_deref()))
                }?;
                handler.call(params
                    .with_args(args)
                    .with_context(self)
                    .into()
                ).await
            },
            _ => Err(Error::from(ErrorCode::ResourceNotFound)),
        }
    }

    #[inline]
    pub(crate) async fn get_prompt(self, params: GetPromptRequestParams) -> Result<GetPromptResult, Error> {
        match self.options.get_prompt(&params.name).await {
            None => Err(Error::new(ErrorCode::InvalidParams, "Prompt not found")),
            Some(prompt) => {
                #[cfg(feature = "http-server")]
                self.validate_claims(prompt.roles.as_deref(), prompt.permissions.as_deref())?;
                prompt.call(params.with_context(self).into()).await
            }
        }
    }

    #[inline]
    pub(crate) async fn call_tool(self, params: CallToolRequestParams) -> Result<CallToolResponse, Error> {
        match self.options.get_tool(&params.name).await {
            None => Err(Error::new(ErrorCode::InvalidParams, "Tool not found")),
            Some(tool) => {
                #[cfg(feature = "http-server")]
                self.validate_claims(tool.roles.as_deref(), tool.permissions.as_deref())?;
                tool.call(params.with_context(self).into()).await
            }
        }
    }

    #[inline]
    #[cfg(feature = "tasks")]
    pub(crate) async fn call_tool_with_task(self, params: CallToolRequestParams) -> Result<ToolOrTaskResponse, Error> {
        match self.options.get_tool(&params.name).await {
            None => Err(Error::new(ErrorCode::InvalidParams, "Tool not found")),
            Some(tool) => {
                #[cfg(feature = "http-server")]
                self.validate_claims(tool.roles.as_deref(), tool.permissions.as_deref())?;
                
                let task_support = tool.task_support();
                if let Some(task_meta) = params.task {
                    self.ensure_tool_augmentation_support(task_support)?;

                    let task = Task::from(task_meta);
                    let handle = self.options.track_task(task.clone());
                    
                    let opt = self.options.clone();
                    let task_id = task.id.clone();
                    tokio::spawn(async move {
                        tokio::select! {
                            result = tool.call(params
                                .with_task(&task_id)
                                .with_context(self).into()) => {
                                let resp = match result {
                                    Ok(result) => {
                                        opt.tasks.complete(&task_id);
                                        result
                                    },
                                    Err(err) => {
                                        opt.tasks.fail(&task_id);
                                        CallToolResponse::error(err)
                                    }
                                };
                                handle.set_result(resp);
                            },
                            _ = handle.cancelled() => {}
                        }
                    });

                    Ok(Either::Left(CreateTaskResult::new(task)))
                } else if task_support.is_some_and(|ts| ts == TaskSupport::Required) {
                    Err(Error::new(
                        ErrorCode::MethodNotFound,
                        "Tool required task augmented call"))
                } else {
                    tool.call(params.with_context(self).into())
                        .await
                        .map(Either::Right)
                }
            }
        }
    }

    /// Requests a list of available roots from a client
    /// 
    /// # Example
    /// ```no_run
    /// # #[cfg(feature = "server-macros")] {
    /// use neva::{Context, error::Error, tool};
    ///
    /// #[tool]
    /// async fn handle_roots(mut ctx: Context) -> Result<(), Error> {
    ///     let roots = ctx.list_roots().await?;
    ///
    ///     // do something with roots
    ///
    /// # Ok(())
    /// }
    /// # }
    /// ```
    pub async fn list_roots(&mut self) -> Result<ListRootsResult, Error> {
        let method = crate::types::root::commands::LIST;
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(ListRootsRequestParams::default()));
        
        self.send_request(req)
            .await?
            .into_result()
    }
    
    /// Sends the sampling request to the client
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(feature = "server-macros")] {
    /// use neva::{
    ///     Context, 
    ///     error::Error, 
    ///     types::sampling::CreateMessageRequestParams, 
    ///     tool
    /// };
    ///
    /// #[tool]
    /// async fn generate_poem(mut ctx: Context, topic: String) -> Result<String, Error> {
    ///     let params = CreateMessageRequestParams::new()
    ///         .with_message(format!("Write a short poem about {topic}"))
    ///         .with_sys_prompt("You are a talented poet who writes concise, evocative verses.");
    /// 
    ///     let result = ctx.sample(params).await?;
    ///     Ok(format!("{:?}", result.content))
    /// }
    /// # }
    /// ```
    #[cfg(not(feature = "tasks"))]
    pub async fn sample(&mut self, params: CreateMessageRequestParams) -> Result<CreateMessageResult, Error> {
        let method = crate::types::sampling::commands::CREATE;
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(params));

        self.send_request(req)
            .await?
            .into_result()
    }

    /// Sends the sampling request to the client
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(feature = "server-macros")] {
    /// use neva::{
    ///     Context, 
    ///     error::Error, 
    ///     types::sampling::CreateMessageRequestParams, 
    ///     tool
    /// };
    ///
    /// #[tool]
    /// async fn generate_poem(mut ctx: Context, topic: String) -> Result<String, Error> {
    ///     let params = CreateMessageRequestParams::new()
    ///         .with_message(format!("Write a short poem about {topic}"))
    ///         .with_sys_prompt("You are a talented poet who writes concise, evocative verses.");
    /// 
    ///     let result = ctx.sample(params).await?;
    ///     Ok(format!("{:?}", result.content))
    /// }
    /// # }
    /// ```
    #[cfg(feature = "tasks")]
    pub async fn sample(&mut self, params: CreateMessageRequestParams) -> Result<CreateMessageResult, Error> {
        let method = crate::types::sampling::commands::CREATE;
        let is_task_aug = params.task.is_some();
        let req = Request::new(
                Some(RequestId::Uuid(uuid::Uuid::new_v4())),
                method,
                Some(params));

        if is_task_aug {
            let result = self.send_request(req)
                .await?
                .into_result()?;

            crate::shared::wait_to_completion(self, result).await
        } else {
            self.send_request(req)
                .await?
                .into_result()
        }
    }

    /// Sends the elicitation request to the client
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(feature = "serve-macros")] {
    /// use neva::{
    ///     Context, 
    ///     error::Error, 
    ///     types::elicitation::ElicitRequestParams, 
    ///     tool
    /// };
    ///
    /// #[tool]
    /// async fn generate_poem(mut ctx: Context, _topic: String) -> Result<String, Error> {
    ///     let params = ElicitRequestParams::new("What is the poem mood you'd like?")
    ///         .with_required("mood", "string");
    ///     let result = ctx.elicit(params).await?;
    ///     Ok(format!("{:?}", result.content))
    /// }
    /// # }
    /// ```
    #[cfg(not(feature = "tasks"))]
    pub async fn elicit(&mut self, params: ElicitRequestParams) -> Result<ElicitResult, Error> {
        let method = crate::types::elicitation::commands::CREATE;
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(params));

        self.send_request(req)
            .await?
            .into_result()
    }

    /// Sends the elicitation request to the client
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(feature = "serve-macros")] {
    /// use neva::{
    ///     Context, 
    ///     error::Error, 
    ///     types::elicitation::ElicitRequestParams, 
    ///     tool
    /// };
    ///
    /// #[tool]
    /// async fn generate_poem(mut ctx: Context, _topic: String) -> Result<String, Error> {
    ///     let params = ElicitRequestParams::new("What is the poem mood you'd like?")
    ///         .with_required("mood", "string");
    ///     let result = ctx.elicit(params).await?;
    ///     Ok(format!("{:?}", result.content))
    /// }
    /// # }
    /// ```
    #[cfg(feature = "tasks")]
    pub async fn elicit(&mut self, params: ElicitRequestParams) -> Result<ElicitResult, Error> {
        let related_task = params.related_task();

        if let Some(related_task) = related_task {
            use std::str::FromStr;

            let task_id = related_task.id;

            let id = RequestId::from_str(&task_id).unwrap();
            let receiver = self.pending.push(&id);

            self.options.tasks.set_result(&task_id, params);
            self.options.tasks.require_input(&task_id);

            let resp = match timeout(self.timeout, receiver).await {
                Ok(Ok(resp)) => resp,
                Ok(Err(_)) => {
                    self.options.tasks.fail(&task_id);
                    return Err(Error::new(ErrorCode::InternalError, "Response channel closed"))
                },
                Err(_) => {
                    _ = self.pending.pop(&id);
                    self.options.tasks.fail(&task_id);
                    return Err(Error::new(ErrorCode::Timeout, "Request timed out"));
                }
            };
            
            self.options.tasks.reset(&task_id);

            return resp.into_result();
        }

        let method = crate::types::elicitation::commands::CREATE;
        let is_task_aug = params.is_task_augmented();
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(params));
        
        if is_task_aug {
            let result = self.send_request(req)
                .await?
                .into_result()?;

            crate::shared::wait_to_completion(self, result).await
        } else {
            self.send_request(req)
                .await?
                .into_result()
        }
    }
    
    /// Notifies the client that the elicitation with the `id` has been completed
    pub async fn complete_elicitation(&mut self, id: impl Into<String>) -> Result<(), Error> {
        let params = serde_json::to_value(ElicitationCompleteParams::new(id)).ok();
        self.send_notification(
            crate::types::elicitation::commands::COMPLETE, 
            params)
            .await
    }

    /// Sends notification that a task with `id` was changed.
    #[cfg(feature = "tasks")]
    pub async fn task_changed(&mut self, id: &str) -> Result<(), Error> {
        let task = self.options.tasks.get_status(id)?;
        let params = serde_json::to_value(task).ok();
        self.send_notification(
            crate::types::task::commands::STATUS, 
            params)
            .await
    }

    /// Applies earlier defined scopes to the current context.
    #[inline]
    #[cfg(feature = "di")]
    pub fn with_scope(mut self, scope: Container) -> Self {
        self.scope = Some(scope);
        self
    }
    
    /// Resolves a service and returns a cloned instance. 
    /// `T` must implement `Clone` otherwise 
    /// use resolve_shared method that returns a shared pointer.
    #[inline]
    #[cfg(feature = "di")]
    pub fn resolve<T: Send + Sync + Clone + 'static>(&self) -> Result<T, Error> {
        self.scope
            .as_ref()
            .ok_or_else(|| Error::new(ErrorCode::InternalError, "DI scope is not set"))?
            .resolve::<T>()
            .map_err(Into::into)
    }

    /// Resolves a service and returns a shared pointer
    #[inline]
    #[cfg(feature = "di")]
    pub fn resolve_shared<T: Send + Sync + 'static>(&self) -> Result<Arc<T>, Error> {
        self.scope
            .as_ref()
            .ok_or_else(|| Error::new(ErrorCode::InternalError, "DI scope is not set"))?
            .resolve_shared::<T>()
            .map_err(Into::into)
    }
    
    #[inline]
    #[cfg(feature = "http-server")]
    fn validate_claims(&self, roles: Option<&[String]>, permissions: Option<&[String]>) -> Result<(), Error> {
        let claims = self.claims.as_ref(); 
        validate_roles(claims, roles)?;
        validate_permissions(claims, permissions)?;
        Ok(())
    }

    #[inline]
    #[cfg(feature = "tasks")]
    fn ensure_tool_augmentation_support(&self, task_support: Option<TaskSupport>) -> Result<(), Error> {
        if !self.options.is_task_augmented_tool_call_supported() {
            return Err(
                Error::new(
                    ErrorCode::MethodNotFound,
                    "Server does not support task augmented tool calls"));
        }
        let Some(task_support) = task_support else {
            return Err(
                Error::new(
                    ErrorCode::MethodNotFound,
                    "Tool does not support task augmented calls"));
        };
        if task_support == TaskSupport::Forbidden {
            return Err(
                Error::new(
                    ErrorCode::MethodNotFound,
                    "Tool forbid task augmented calls"));
        }
        Ok(())
    }
    
    /// Sends a [`Request`] to a client
    #[inline]
    async fn send_request(&mut self, mut req: Request) -> Result<Response, Error> {
        if let Some(session_id) = self.session_id {
            req.session_id = Some(session_id);
        }

        let id = req.full_id();
        let receiver = self.pending.push(&id);
        self.sender.send(req.into()).await?;

        match timeout(self.timeout, receiver).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => Err(Error::new(ErrorCode::InternalError, "Response channel closed")),
            Err(_) => {
                _ = self.pending.pop(&id);
                Err(Error::new(ErrorCode::Timeout, "Request timed out"))
            }
        }
    }

    /// Sends a notification to a client
    #[inline]
    async fn send_notification(
        &mut self, 
        method: &str, 
        params: Option<serde_json::Value>
    ) -> Result<(), Error> {
        let mut notification = Notification::new(method, params);
        if let Some(session_id) = self.session_id {
            notification.session_id = Some(session_id);
        }
        self.sender.send(notification.into()).await
    }
}

#[cfg(feature = "tasks")]
impl crate::shared::TaskApi for Context {
    /// Retrieve task result from the client. If the task is not completed yet, waits until it completes or cancels.
    async fn get_task_result<T>(&mut self, id: impl Into<String>) -> Result<T, Error>
    where 
        T: DeserializeOwned
    {
        let params = GetTaskPayloadRequestParams { id: id.into() };
        let method = crate::types::task::commands::RESULT;
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(params));

        self.send_request(req)
            .await?
            .into_result()
    }

    /// Retrieve task status from the client
    async fn get_task(&mut self, id: impl Into<String>) -> Result<Task, Error> {
        let params = GetTaskRequestParams { id: id.into() };
        let method = crate::types::task::commands::GET;
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(params));
        
        self.send_request(req)
            .await?
            .into_result()
    }
    
    /// Cancels a task that is currently running on the client
    async fn cancel_task(&mut self, id: impl Into<String>) -> Result<Task, Error> {       
        if !self.options.is_tasks_cancellation_supported() {
            return Err(Error::new(
                ErrorCode::InvalidRequest, 
                "Server does not support cancelling tasks."));
        }

        let params = CancelTaskRequestParams { id: id.into() };
        let method = crate::types::task::commands::CANCEL;
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(params));
        
        self.send_request(req)
            .await?
            .into_result()
    }

    /// Retrieves a list of tasks from the client
    async fn list_tasks(&mut self, cursor: Option<Cursor>) -> Result<ListTasksResult, Error> {

        if !self.options.is_tasks_list_supported() {
            return Err(Error::new(
                ErrorCode::InvalidRequest, 
                "Server does not support retrieving a task list."));
        }

        let params = ListTasksRequestParams { cursor };
        let method = crate::types::task::commands::LIST;
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(params));
        
        self.send_request(req)
            .await?
            .into_result()
    }

    async fn handle_input(&mut self, _id: &str, _params: TaskPayload) -> Result<(), Error> {
        // Reserved, there are no cases so far, for the server 
        // to handle input requests from client.
        Ok(())
    }
}
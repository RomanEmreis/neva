//! Server runtime context utilities

use tokio::time::timeout;
use crate::error::{Error, ErrorCode};
use crate::transport::Sender;
use super::{options::{McpOptions, RuntimeMcpOptions}, handler::RequestHandler};
use crate::{
    shared::RequestQueue,
    middleware::{MwContext, Next},
    transport::TransportProtoSender,
    types::{
        Tool, CallToolRequestParams, CallToolResponse,
        Resource, ReadResourceRequestParams, ReadResourceResult,
        Prompt, GetPromptRequestParams, GetPromptResult,
        RequestId, Request, Response, Uri,
        Message,
        notification::Notification,
        root::{ListRootsRequestParams, ListRootsResult},
        resource::SubscribeRequestParams,
        sampling::{CreateMessageRequestParams, CreateMessageResult},
        elicitation::{ElicitRequestParams, ElicitResult}
    },
};
use std::{
    collections::HashMap,
    time::Duration,
    sync::Arc
};
use std::fmt::{Debug, Formatter};
#[cfg(feature = "http-server")]
use {
    crate::transport::http::server::{validate_roles, validate_permissions},
    crate::auth::DefaultClaims,
    volga::headers::HeaderMap
};

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
    mw_start: Option<Next>
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
    ) -> Self {
        let middlewares = options.middlewares.take();
        Self {
            pending: Default::default(),
            handlers: Arc::new(handlers),
            options: options.into_runtime(),
            mw_start: middlewares.and_then(|mw| mw.compose()),
            sender,
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
    /// Reads a resource contents
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(feature = "server-macros")] {
    /// use neva::{Context, error::Error, types::Uri, tool};
    ///
    /// #[tool]
    /// async fn summarize_document(mut ctx: Context, doc_uri: Uri) -> Result<(), Error> {
    ///     let doc = ctx.resource(doc_uri).await?;
    ///     
    ///     // do something with the doc
    ///
    /// # Ok(())
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
    
    #[inline]
    #[cfg(feature = "http-server")]
    fn validate_claims(&self, roles: Option<&[String]>, permissions: Option<&[String]>) -> Result<(), Error> {
        let claims = self.claims.as_ref(); 
        validate_roles(claims, roles)?;
        validate_permissions(claims, permissions)?;
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

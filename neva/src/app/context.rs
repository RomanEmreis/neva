//! Server runtime context utilities

use tokio::time::timeout;
use crate::error::{Error, ErrorCode};
use crate::transport::Sender;
use super::{options::{McpOptions, RuntimeMcpOptions}, handler::RequestHandler};
use crate::{
    shared::RequestQueue,
    transport::TransportProtoSender,
    types::{
        Tool, CallToolRequestParams, CallToolResponse,
        Resource, ReadResourceRequestParams, ReadResourceResult,
        Prompt, GetPromptRequestParams, GetPromptResult,
        RequestId, Request, Response, Uri,
        resource::Route,
        notification::Notification,
        root::{ListRootsRequestParams, ListRootsResult},
        resource::SubscribeRequestParams,
        sampling::{CreateMessageRequestParams, CreateMessageResult}
    },
};
use std::{
    collections::HashMap,
    time::Duration,
    sync::Arc
};

type RequestHandlers = HashMap<String, RequestHandler<Response>>;

/// Represents a Server runtime
#[derive(Clone)]
pub(crate) struct ServerRuntime {
    options: RuntimeMcpOptions,
    handlers: Arc<RequestHandlers>,
    pending: RequestQueue,
    sender: TransportProtoSender,
}

/// Represents MCP Request Context
#[derive(Clone)]
pub struct Context {
    pub(crate) options: RuntimeMcpOptions,
    pending: RequestQueue,
    sender: TransportProtoSender,
    timeout: Duration,
}

impl ServerRuntime {
    /// Creates a new server runtime
    pub(crate) fn new(
        sender: TransportProtoSender, 
        options: McpOptions,
        handlers: RequestHandlers,
    ) -> Self {
        Self {
            pending: Default::default(),
            handlers: Arc::new(handlers),
            options: options.into_runtime(),
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
    pub(crate) fn context(&self) -> Context {
        Context {
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
}

impl Context {
    /// Reads a resource contents
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(feature = "macros")] {
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
            .read_resource(params).await
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
        if self.is_subscribed(&uri).await {
            let params = serde_json::to_value(SubscribeRequestParams::from(uri)).ok();
            self.send_notification(crate::types::resource::commands::UPDATED, params).await   
        } else { 
            Ok(())
        }
    }

    /// Adds a subscription to the resource with the [`Uri`]
    pub async fn subscribe_to_resource(&mut self, uri: impl Into<Uri>) {
        self.options
            .resource_subscriptions
            .write()
            .await
            .insert(uri.into());
    }
    
    /// Removes a subscription to the resource with the [`Uri`]
    pub async fn unsubscribe_from_resource(&mut self, uri: &Uri) {
        self.options
            .resource_subscriptions
            .write()
            .await
            .remove(uri);
    }
    
    /// Returns `true` if there is a subscription to changes of the resource with the [`Uri`]
    pub async fn is_subscribed(&self, uri: &Uri) -> bool {
        self.options
            .resource_subscriptions
            .read()
            .await
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
        let params = params.with_context(self);
        match opt.read_resource(&params.uri) {
            Some(Route::Handler(handler)) => handler
                .call(params.into()).await,
            _ => Err(Error::from(ErrorCode::ResourceNotFound)),
        }
    }

    #[inline]
    pub(crate) async fn get_prompt(self, params: GetPromptRequestParams) -> Result<GetPromptResult, Error> {
        let opt = self.options.clone();
        let params = params.with_context(self);
        match opt.get_prompt(&params.name).await {
            Some(prompt) => prompt.call(params.into()).await,
            None => Err(Error::new(ErrorCode::InvalidParams, "Prompt not found"))
        }
    }

    #[inline]
    pub(crate) async fn call_tool(self, params: CallToolRequestParams) -> Result<CallToolResponse, Error> {
        let opt = self.options.clone();
        let params = params.with_context(self);
        match opt.get_tool(&params.name).await {
            Some(tool) => tool.call(params.into()).await,
            None => Err(Error::new(ErrorCode::InvalidParams, "Tool not found"))
        }
    }
    
    /// Requests a list of available roots from a client
    /// 
    /// # Example
    /// ```no_run
    /// # #[cfg(feature = "macros")] {
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
        let id = RequestId::String(method.into());
        let req = Request::new(
            Some(id.clone()),
            method,
            Some(ListRootsRequestParams::default()));

        self.send_request(&id, req)
            .await?
            .into_result()
    }
    
    /// Send a sampling request to the client
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(feature = "macros")] {
    /// use neva::{
    ///     Context, 
    ///     error::Error, 
    ///     types::sampling::CreateMessageRequestParams, 
    ///     tool
    /// };
    ///
    /// #[tool]
    /// async fn generate_poem(mut ctx: Context, topic: String) -> Result<String, Error> {
    ///     let params = CreateMessageRequestParams::message(
    ///         &format!("Write a short poem about {topic}"),
    ///         "You are a talented poet who writes concise, evocative verses."
    ///     );
    ///     let result = ctx.sample(params).await?;
    ///
    ///     Ok(format!("{:?}", result.content.text))
    /// }
    /// # }
    /// ```
    pub async fn sample(&mut self, params: CreateMessageRequestParams) -> Result<CreateMessageResult, Error> {
        let method = crate::types::sampling::commands::CREATE;
        let id = RequestId::String(method.into());
        let req = Request::new(
            Some(id.clone()),
            method,
            Some(params));

        self.send_request(&id, req)
            .await?
            .into_result()
    }
    
    /// Sends a [`Request`] to a client
    #[inline]
    async fn send_request(&mut self, id: &RequestId, req: Request) -> Result<Response, Error> {
        let receiver = self.pending.push(id).await;
        self.sender.send(req.into()).await?;

        match timeout(self.timeout, receiver).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => Err(Error::new(ErrorCode::InternalError, "Response channel closed")),
            Err(_) => {
                _ = self.pending.pop(id).await;
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
        let notification = Notification::new(method, params);
        self.sender.send(notification.into()).await
    }
}
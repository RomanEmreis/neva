//! Utilities for the MCP client

use std::{collections::HashMap, future::Future, sync::Arc};
use options::McpOptions;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use handler::RequestHandler;
use crate::error::{Error, ErrorCode};
use crate::shared;
use crate::transport::Transport;
use crate::types::{
    ListToolsRequestParams, ListToolsResult, CallToolRequestParams, CallToolResponse,
    ListResourcesRequestParams, ListResourcesResult, ReadResourceRequestParams, ReadResourceResult,
    ListResourceTemplatesRequestParams, ListResourceTemplatesResult, Uri, 
    ListPromptsRequestParams, ListPromptsResult, GetPromptRequestParams, GetPromptResult,
    ServerCapabilities, ClientCapabilities, Implementation, InitializeRequestParams, InitializeResult, 
    Request, RequestId, Response, request::RequestParamsMeta, cursor::Cursor,
    notification::Notification, 
    resource::{SubscribeRequestParams, UnsubscribeRequestParams}, 
    sampling::{CreateMessageRequestParams, CreateMessageResult, SamplingHandler},
    elicitation::{ElicitRequestParams, ElicitResult, ElicitationHandler},
    Root
};

mod handler;
mod notification_handler;
pub mod options;
pub mod subscribe;

/// Represents an MCP client app 
pub struct Client {
    /// MCP client options.
    options: McpOptions,

    /// Capabilities supported by the connected server.
    server_capabilities: Option<ServerCapabilities>,
    
    /// Implementation information of the connected server.
    server_info: Option<Implementation>,
    
    /// A [`CancellationToken`] that cancels transport background processes.
    cancellation_token: Option<CancellationToken>,
    
    /// Request handler
    handler: Option<RequestHandler>
}

impl Default for Client {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    /// Initializes a new client app
    pub fn new() -> Self {
        Self {
            options: McpOptions::default(),
            server_capabilities: None,
            server_info: None,
            cancellation_token: None,
            handler: None
        }
    }

    /// Configure MCP client options
    pub fn with_options<F>(mut self, config: F) -> Self
    where
        F: FnOnce(McpOptions) -> McpOptions
    {
        self.options = config(self.options);
        self
    }
    
    /// Adds a new Root
    /// 
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// # use neva::error::Error;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Error> {
    /// let mut client = Client::new();
    /// client.add_root("file:///home/user/projects/my_project", "My Project");
    /// # client.disconnect().await
    /// # }
    /// ```    
    pub fn add_root(&mut self, uri: &str, name: &str) -> &mut Self {
        self.options.add_root(Root::new(uri, name));
        self.publish_roots_changed();
        self
    }

    /// Adds multiple new Roots.
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// # use neva::error::Error;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Error> {
    /// let mut client = Client::new();
    /// client.add_roots([
    ///     ("file:///home/user/projects/my_project", "My Project"),
    ///     ("file:///home/user/projects/another_project", "My Another Project")
    /// ]);
    /// # client.disconnect().await
    /// # }
    /// ```    
    pub fn add_roots<T, I>(&mut self, roots: I) -> &mut Self
    where 
        T: Into<Root>,
        I: IntoIterator<Item = T>,
    {
        self.options.add_roots(roots);
        self.publish_roots_changed();
        self
    }
    
    /// Sends the "notifications/roots/list_changed" notification to the server
    pub fn publish_roots_changed(&mut self) {
        if let Some(handler) = self.handler.as_mut() {
            let roots = self.options.roots();
            handler.notify_roots_changed(roots);
        }
    }

    /// Registers a handler that will be running when a "sampling/createMessage" request is received
    pub fn map_sampling<F, R>(&mut self, handler: F) -> &mut Self
    where
        F: Fn(CreateMessageRequestParams) -> R + Clone + Send + Sync + 'static,
        R: Future + Send,
        R::Output: Into<CreateMessageResult>,
    {
        let handler: SamplingHandler = make_handler(handler);
        self.options.add_sampling_handler(handler);
        self
    }

    /// Registers a handler that will be running when an "elicitation/create" request is received
    pub fn map_elicitation<F, R>(&mut self, handler: F) -> &mut Self
    where
        F: Fn(ElicitRequestParams) -> R + Clone + Send + Sync + 'static,
        R: Future + Send,
        R::Output: Into<ElicitResult>,
    {
        let handler: ElicitationHandler = make_handler(handler);
        self.options.add_elicitation_handler(handler);
        self
    }
    
    /// Connects the MCP client to the MCP server
    /// 
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    /// 
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    /// 
    ///     client.connect().await?;
    /// 
    ///     // call tools, read resources, etc.
    /// 
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn connect(&mut self) -> Result<(), Error> {
        #[cfg(feature = "macros")]
        self.register_methods();
        
        let mut transport = self.options.transport();
        let token = transport.start();
        
        #[cfg(feature = "tracing")]
        self.register_tracing_notification_handlers();
        
        self.cancellation_token = Some(token);
        self.handler = Some(RequestHandler::new(transport, &self.options));
        
        self.wait_for_shutdown_signal();
        self.init().await
    }

    /// Disconnects the MCP client from the MCP server
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     // call tools, read resources, etc.
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn disconnect(mut self) -> Result<(), Error> {
        self.send_notification(crate::types::notification::commands::CANCELLED, None).await?;
        if let Some(token) = self.cancellation_token {
            token.cancel();
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        Ok(())
    }
    
    /// Sends `initialize` request to an MCP server
    pub async fn init(&mut self) -> Result<(), Error> {
        let params = InitializeRequestParams {
            protocol_ver: self.options.protocol_ver().to_string(),
            client_info: Some(self.options.implementation.clone()),
            capabilities: Some(ClientCapabilities {
                roots: self.options.roots_capability(),
                sampling: self.options.sampling_capability(),
                elicitation: self.options.elicitation_capability(),
                experimental: None,
            })
        };

        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())), 
            crate::commands::INIT, 
            Some(params));
        
        let resp = self.send_request(req).await?;

        let init_result = resp.into_result::<InitializeResult>()?;

        assert_eq!(
            init_result.protocol_ver,
            self.options.protocol_ver(),
            "Server protocol version mismatch.");
        
        self.server_capabilities = Some(init_result.capabilities);
        self.server_info = Some(init_result.server_info);

        self.send_notification(crate::types::notification::commands::INITIALIZED, None).await
    }
    
    /// Requests a list of tools that MCP server provides
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     // Fetch all or initial list of tools if the MCP server provides pagination
    ///     let tools = client.list_tools(None).await?;
    ///     
    ///     // Fetch the next page of tools is any   
    ///     let tools = client.list_tools(tools.next_cursor).await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ``` 
    pub async fn list_tools(&mut self, cursor: Option<Cursor>) -> Result<ListToolsResult, Error> {
        let id = self.generate_id()?;
        let request = Request::new(
            Some(id),
            crate::types::tool::commands::LIST,
            Some(ListToolsRequestParams { cursor }));

        self.send_request(request)
            .await?
            .into_result()
    }

    /// Requests a list of resources that MCP server provides
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     // Fetch all or initial list of resources if the MCP server provides pagination
    ///     let resources = client.list_resources(None).await?;
    ///     
    ///     // Fetch the next page of resources is any   
    ///     let resources = client.list_resources(resources.next_cursor).await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ``` 
    pub async fn list_resources(&mut self, cursor: Option<Cursor>) -> Result<ListResourcesResult, Error> {
        let id = self.generate_id()?;
        let request = Request::new(
            Some(id),
            crate::types::resource::commands::LIST, 
            Some(ListResourcesRequestParams { cursor }));

        self.send_request(request)
            .await?
            .into_result()
    }

    /// Requests a list of resource templates that MCP server provides
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     // Fetch all or initial list of resource templates if the MCP server provides pagination
    ///     let templates = client.list_resource_templates(None).await?;
    ///     
    ///     // Fetch the next page of resource templates is any   
    ///     let templates = client.list_resource_templates(templates.next_cursor).await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ``` 
    pub async fn list_resource_templates(&mut self, cursor: Option<Cursor>) -> Result<ListResourceTemplatesResult, Error> {
        let id = self.generate_id()?;
        let request = Request::new(
            Some(id),
            crate::types::resource::commands::TEMPLATES_LIST,
            Some(ListResourceTemplatesRequestParams { cursor }));

        self.send_request(request)
            .await?
            .into_result()
    }

    /// Requests a list of prompts that MCP server provides
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     // Fetch all or initial list of prompts if the MCP server provides pagination
    ///     let prompts = client.list_prompts(None).await?;
    ///     
    ///     // Fetch the next page of prompts templates is any   
    ///     let prompts = client.list_prompts(prompts.next_cursor).await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ``` 
    pub async fn list_prompts(&mut self, cursor: Option<Cursor>) -> Result<ListPromptsResult, Error> {
        let id = self.generate_id()?;
        let request = Request::new(
            Some(id),
            crate::types::prompt::commands::LIST,
            Some(ListPromptsRequestParams { cursor }));

        self.send_request(request)
            .await?
            .into_result()
    }

    /// Calls a tool that MCP server supports
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     let args = [("message", "Hello MCP!")]; // or let args = ("message", "Hello MCP!"); 
    ///     let result = client.call_tool("echo", args).await?;
    ///     // Do something with the result
    ///
    ///     client.disconnect().await
    /// }
    /// ``` 
    pub async fn call_tool<Args: IntoArgs>(&mut self, name: &str, args: Args) -> Result<CallToolResponse, Error> {
        let id = self.generate_id()?;
        let request = Request::new(
            Some(id.clone()),
            crate::types::tool::commands::CALL,
            Some(CallToolRequestParams {
                name: name.into(),
                meta: Some(RequestParamsMeta::new(&id)),
                args: args.into_args()
            }));
        
        self.send_request(request)
            .await?
            .into_result()
    }

    /// Requests resource contents from MCP server
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     let resource = client.read_resource("res://res_1").await?;
    ///     // Do something with the resource
    ///
    ///     client.disconnect().await
    /// }
    /// ``` 
    pub async fn read_resource(&mut self, uri: impl Into<Uri>) -> Result<ReadResourceResult, Error> {
        let id = self.generate_id()?;
        let request = Request::new(
            Some(id.clone()),
            crate::types::resource::commands::READ,
            Some(ReadResourceRequestParams {
                uri: uri.into(),
                meta: Some(RequestParamsMeta::new(&id)),
                #[cfg(feature = "server")]
                args: None
            })
        );

        self.send_request(request)
            .await?
            .into_result()
    }

    /// Gets a prompt that MCP server provides
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     let args = [
    ///         ("temperature", "50"),
    ///         ("style", "anything")
    ///     ];
    ///     let prompt = client.get_prompt("complex_prompt", args).await?;
    ///     // Do something with the prompt
    ///
    ///     client.disconnect().await
    /// }
    /// ``` 
    pub async fn get_prompt<Args: IntoArgs>(&mut self, name: &str, args: Args) -> Result<GetPromptResult, Error> {
        let id = self.generate_id()?;
        let request = Request::new(
            Some(id.clone()),
            crate::types::prompt::commands::GET,
            Some(GetPromptRequestParams {
                name: name.into(),
                meta: Some(RequestParamsMeta::new(&id)),
                args: args.into_args()
            })
        );

        self.send_request(request)
            .await?
            .into_result()
    }
    
    /// Subscribes to a resource on the server to receive notifications when it changes.
    pub async fn subscribe_to_resource(&mut self, uri: impl Into<Uri>) -> Result<(), Error> {
        if !self.is_resource_subscription_supported() {
            return Err(Error::new(
                ErrorCode::MethodNotFound,
                "Server does not support resource subscriptions",
            ));
        }
        
        let id = self.generate_id()?;
        let request = Request::new(
            Some(id.clone()),
            crate::types::resource::commands::SUBSCRIBE,
            Some(SubscribeRequestParams::from(uri))
        );
        let response = self.send_request(request).await?;
        match response {
            Response::Ok(_) => Ok(()),
            Response::Err(err) => Err(err.error.into()),
        }
    }

    /// Unsubscribes from a resource on the server to stop receiving notifications about its changes.
    pub async fn unsubscribe_from_resource(&mut self, uri: impl Into<Uri>) -> Result<(), Error> {
        if !self.is_resource_subscription_supported() {
            return Err(Error::new(
                ErrorCode::MethodNotFound,
                "Server does not support resource subscriptions",
            ));
        }

        let id = self.generate_id()?;
        let request = Request::new(
            Some(id.clone()),
            crate::types::resource::commands::UNSUBSCRIBE,
            Some(UnsubscribeRequestParams::from(uri))
        );

        let response = self.send_request(request).await?;
        match response {
            Response::Ok(_) => Ok(()),
            Response::Err(err) => Err(err.error.into()),
        }
    }
    
    /// Maps the `handler` to a specific `event`
    pub fn subscribe<F, R>(&mut self, event: &str, handler: F)
    where
        F: Fn(Notification) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = ()> + Send
    {
        self.options
            .notification_handler
            .get_or_insert_default()
            .subscribe(event, handler);
    }
    
    /// Unsubscribe a handler from the `event`
    pub fn unsubscribe(&mut self, event: &str) {
        if let Some(notification_handler) = &self.options.notification_handler {
            notification_handler.unsubscribe(event);
        } 
    }

    /// Returns whether the server is configured to send the "notifications/resources/updated"
    #[inline]
    fn is_resource_subscription_supported(&self) -> bool {
        self.server_capabilities
            .as_ref()
            .and_then(|cap| cap.resources.as_ref())
            .is_some_and(|res| res.subscribe)
    }

    /// Returns whether the server is configured to send the "notifications/resources/list_changed"
    #[inline]
    fn is_resource_list_changed_supported(&self) -> bool {
        self.server_capabilities
            .as_ref()
            .and_then(|cap| cap.resources.as_ref())
            .is_some_and(|res| res.list_changed)
    }

    /// Returns whether the server is configured to send the "notifications/tools/list_changed"
    #[inline]
    fn is_tools_list_changed_supported(&self) -> bool {
        self.server_capabilities
            .as_ref()
            .and_then(|cap| cap.tools.as_ref())
            .is_some_and(|tool| tool.list_changed)
    }

    /// Returns whether the server is configured to send the "notifications/prompts/list_changed"
    #[inline]
    fn is_prompts_list_changed_supported(&self) -> bool {
        self.server_capabilities
            .as_ref()
            .and_then(|cap| cap.prompts.as_ref())
            .is_some_and(|prompt| prompt.list_changed)
    }

    /// Sends a request to the MCP server
    #[inline]
    async fn send_request(&mut self, req: Request) -> Result<Response, Error> {
        self.handler.as_mut()
            .ok_or_else(|| Error::new(ErrorCode::InternalError, "Connection closed"))?
            .send_request(req)
            .await
    }
    
    /// Sends a notification to the MCP server
    #[inline]
    async fn send_notification(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>
    ) -> Result<(), Error> {
        let notification = Notification::new(method, params);
        self.handler.as_mut()
            .ok_or_else(|| Error::new(ErrorCode::InternalError, "Connection closed"))?
            .send_notification(notification)
            .await
    }

    #[cfg(feature = "tracing")]
    fn register_tracing_notification_handlers(&mut self) {
        use crate::types::notification::commands::*;
        
        self.subscribe(MESSAGE, Self::default_notification_handler);
        self.subscribe(STDERR, Self::default_notification_handler);
        self.subscribe(PROGRESS, Self::default_notification_handler);
    }
    
    #[cfg(feature = "tracing")]
    async fn default_notification_handler(notification: Notification) {
        notification.write();
    }

    /// Generates a new [`RequestId`]
    #[inline]
    fn generate_id(&self) -> Result<RequestId, Error> {
        self.handler.as_ref()
            .ok_or_else(|| Error::new(ErrorCode::InternalError, "Connection closed"))
            .map(|h| h.next_id())
    }
    
    #[inline]
    fn wait_for_shutdown_signal(&mut self) {
        if let Some(token) = self.cancellation_token.clone() {
            shared::wait_for_shutdown_signal(token);
        };
    }
}

/// A trait describes arguments for tools and prompts
pub trait IntoArgs {
    fn into_args(self) -> Option<HashMap<String, serde_json::Value>>;
}

impl IntoArgs for () {
    #[inline]
    fn into_args(self) -> Option<HashMap<String, serde_json::Value>> {
        None
    }
}

impl<T: IntoArgs> IntoArgs for Option<T> {
    #[inline]
    fn into_args(self) -> Option<HashMap<String, serde_json::Value>> {
        self.and_then(|args| args.into_args())
    }
}

impl<K, T> IntoArgs for (K, T)
where
    K: Into<String>,
    T: Serialize,
{
    #[inline]
    fn into_args(self) -> Option<HashMap<String, serde_json::Value>> {
        Some(HashMap::from([
            (self.0.into(), serde_json::to_value(self.1).unwrap())
        ]))
    }
}

impl<K, T, const N: usize> IntoArgs for [(K, T); N]
where
    K: Into<String>,
    T: Serialize,
{
    #[inline]
    fn into_args(self) -> Option<HashMap<String, serde_json::Value>> {
        Some(make_args(self))
    }
}

impl<K, T> IntoArgs for Vec<(K, T)>
where
    K: Into<String>,
    T: Serialize,
{
    #[inline]
    fn into_args(self) -> Option<HashMap<String, serde_json::Value>> {
        Some(make_args(self))
    }
}

impl<K, T> IntoArgs for HashMap<K, T>
where
    K: Into<String>,
    T: Serialize,
{
    #[inline]
    fn into_args(self) -> Option<HashMap<String, serde_json::Value>> {
        Some(make_args(self))
    }
}

/// Creates arguments for tools and prompts from iterator
#[inline]
fn make_args<I, K, T>(args: I) -> HashMap<String, serde_json::Value>
where
    I: IntoIterator<Item = (K, T)>,
    K: Into<String>,
    T: Serialize,
{
    HashMap::from_iter(args
        .into_iter()
        .map(|(k, v)| (k.into(), serde_json::to_value(v).unwrap())))
}

#[inline]
fn make_handler<F, R, P, O>(handler: F) -> Handler<P, O>
where
    F: Fn(P) -> R + Clone + Send + Sync + 'static,
    R: Future + Send,
    R::Output: Into<O>,
    P: Send + 'static,
    O: Send + 'static,
{
    Arc::new(move |params: P| {
        let handler = handler.clone();
        Box::pin(async move { handler(params).await.into() })
    })
}

type Handler<P, O> = Arc<
    dyn Fn(P) -> std::pin::Pin<Box<dyn Future<Output = O> + Send>>
    + Send
    + Sync
>;

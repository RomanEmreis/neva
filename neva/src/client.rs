//! Utilities for the MCP client

use std::{future::Future, sync::Arc};
use std::fmt::{Debug, Formatter};
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
    Request, RequestId, Response, RequestParamsMeta, 
    cursor::Cursor,
    notification::Notification, 
    resource::{SubscribeRequestParams, UnsubscribeRequestParams}, 
    sampling::{CreateMessageRequestParams, CreateMessageResult, SamplingHandler},
    elicitation::{ElicitRequestParams, ElicitResult, ElicitationHandler},
    Root
};

#[cfg(feature = "tasks")]
use serde::de::DeserializeOwned;

#[cfg(feature = "tasks")]
use crate::{
    types::{
        Task, TaskStatus, TaskPayload,
        ListTasksRequestParams, ListTasksResult,
        GetTaskPayloadRequestParams,
        CancelTaskRequestParams,
        GetTaskRequestParams,
        CreateTaskResult,
        TaskMetadata, 
    },
    shared::Either
};

mod handler;
mod notification_handler;
pub mod options;
pub mod subscribe;

#[cfg(feature = "tasks")]
const DEFAULT_POLL_INTERVAL: usize = 5000; // 5 seconds

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

impl Debug for Client {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("options", &self.options)
            .field("server_capabilities", &self.server_capabilities)
            .field("server_info", &self.server_info)
            .finish()
    }
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
    pub fn add_root(&mut self, uri: impl Into<Uri>, name: impl Into<String>) -> &mut Self {
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
                #[cfg(feature = "tasks")]
                tasks: self.options.tasks_capability(),
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
    
    /// Sends a ping to the MCP server
    pub async fn ping(&mut self) -> Result<Response, Error> {
        self.command::<()>(crate::commands::PING, None).await
    }
    
    /// Sends a command to the MCP server
    /// 
    /// # Example
    /// ```no_run
    /// use neva::prelude::*;
    /// 
    /// #[derive(serde::Serialize)]
    /// struct MyCommandParams {
    ///     param: String,
    /// }
    /// 
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     let params = MyCommandParams { param: "Hello MCP!".to_string() };
    ///     let tools = client.command("my-command", Some(params)).await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    #[inline]
    pub async fn command<T: Serialize>(
        &mut self, 
        command: impl Into<String>, 
        params: Option<T>
    ) -> Result<Response, Error> {
        let id = self.generate_id()?;
        let request = Request::new(Some(id), command, params);
        self.send_request(request).await
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
        let params = ListToolsRequestParams { cursor };
        self.command(crate::types::tool::commands::LIST, Some(params))
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
        let params = ListResourcesRequestParams { cursor };
        self.command(crate::types::resource::commands::LIST, Some(params))
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
        let params = ListResourceTemplatesRequestParams { cursor };
        self.command(crate::types::resource::commands::TEMPLATES_LIST, Some(params))
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
        let params = ListPromptsRequestParams { cursor };
        self.command(crate::types::prompt::commands::LIST, Some(params))
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
    ///
    /// # Structured output
    /// ```no_run
    /// use neva::prelude::*;
    /// 
    /// #[json_schema(de)]
    /// struct Weather {
    ///     conditions: String,
    ///     temperature: f32,
    ///     humidity: f32,
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     let tools = client.list_tools(None).await?;
    /// 
    ///     // Get the tool by name
    ///     let tool: &Tool = tools.get("weather-forecast")
    ///         .expect("Weather forecast tool not found");
    /// 
    ///     let args = ("location", "London");
    ///     let result = client.call_tool("weather-forecast", args).await?;
    /// 
    ///     // Validate the output structure and deserialize the result
    ///     let weather: Weather = tool
    ///         .validate(&result)
    ///         .and_then(|res| res.as_json())?;
    ///     
    ///     // Do something with the result
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn call_tool<N, Args>(
        &mut self, 
        name: N, 
        args: Args
    ) -> Result<CallToolResponse, Error>
    where
        N: Into<String>,
        Args: shared::IntoArgs
    {
        let id = self.generate_id()?;
        let request = Request::new(
            Some(id.clone()),
            crate::types::tool::commands::CALL,
            Some(CallToolRequestParams {
                name: name.into(),
                meta: Some(RequestParamsMeta::new(&id)),
                args: args.into_args(),
                #[cfg(feature = "tasks")]
                task: None
            }));
        
        self.send_request(request)
            .await?
            .into_result()
    }

    /// Calls a task-augmented tool that MCP server supports
    ///
    /// # Panics
    /// If the server does not support task-augmented tool calls
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
    ///     let result = client.call_tool_with_task("echo", args, None).await?;
    ///     // Do something with the result
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    ///
    /// # Structured output
    /// ```no_run
    /// use neva::prelude::*;
    ///
    /// #[json_schema(de)]
    /// struct Weather {
    ///     conditions: String,
    ///     temperature: f32,
    ///     humidity: f32,
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     let tools = client.list_tools(None).await?;
    ///
    ///     // Get the tool by name
    ///     let tool: &Tool = tools.get("weather-forecast")
    ///         .expect("Weather forecast tool not found");
    ///
    ///     let args = ("location", "London");
    ///     let result = client.call_tool_with_task("weather-forecast", args, None).await?;
    ///
    ///     // Validate the output structure and deserialize the result
    ///     let weather: Weather = tool
    ///         .validate(&result)
    ///         .and_then(|res| res.as_json())?;
    ///     
    ///     // Do something with the result
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    #[cfg(feature = "tasks")]
    pub async fn call_tool_with_task<N, Args>(
        &mut self,
        name: N,
        args: Args,
        ttl: Option<usize>
    ) -> Result<CallToolResponse, Error>
    where
        N: Into<String>,
        Args: shared::IntoArgs
    {
        assert!(
            self.is_server_support_call_tool_with_tasks(), 
            "Server does not support call tool with tasks.");
        
        let id = self.generate_id()?;
        let request = Request::new(
            Some(id.clone()),
            crate::types::tool::commands::CALL,
            Some(CallToolRequestParams {
                name: name.into(),
                meta: Some(RequestParamsMeta::new(&id)),
                args: args.into_args(),
                task: Some(TaskMetadata { ttl })
            }));

        let result = self.send_request(request)
            .await?
            .into_result::<Either<CreateTaskResult, CallToolResponse>>()?;

        let mut task = match result {
            Either::Right(result) => return Ok(result),
            Either::Left(task_result) => task_result.task,
        };
        
        let mut elapsed = 0;
        
        loop {
            if ttl.is_some_and(|ttl| ttl <= elapsed) {
                #[cfg(feature = "tracing")]
                tracing::debug!(logger = "neva", "Task TTL expired. Cancelling task.");
                
                let _ = self.cancel_task(&task.id).await?;
                return Err(Error::new(ErrorCode::InvalidRequest, "Task was cancelled: TTL expired"));
            }
            
            task = self.get_task(&task.id).await?;
            
            if task.status == TaskStatus::Completed {
                let result: TaskPayload<CallToolResponse> = self
                    .get_task_result(&task.id)
                    .await?;
                return Ok(result.into_inner());
            } else {
                let poll_interval = task
                    .poll_interval
                    .unwrap_or(DEFAULT_POLL_INTERVAL);
                
                elapsed += poll_interval;
                
                #[cfg(feature = "tracing")]
                tracing::debug!(logger = "neva", "Waiting for task to complete. Elapsed: {elapsed}ms");
                
                tokio::time::sleep(std::time::Duration::from_millis(poll_interval as u64)).await;   
            }
        }
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
    pub async fn get_prompt<N, Args>(
        &mut self, 
        name: N,
        args: Args
    ) -> Result<GetPromptResult, Error>
    where
        N: Into<String>,
        Args: shared::IntoArgs
    {
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
        
        let params = SubscribeRequestParams::from(uri);
        let resp = self
            .command(crate::types::resource::commands::SUBSCRIBE, Some(params))
            .await?;
        
        match resp {
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

        let params = UnsubscribeRequestParams::from(uri);
        let resp = self
            .command(crate::types::resource::commands::UNSUBSCRIBE, Some(params))
            .await?;
        
        match resp {
            Response::Ok(_) => Ok(()),
            Response::Err(err) => Err(err.error.into()),
        }
    }
    
    /// Maps the `handler` to a specific `event`
    pub fn subscribe<E, F, R>(&mut self, event: E, handler: F)
    where
        E: Into<String>,
        F: Fn(Notification) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = ()> + Send
    {
        self.options
            .notification_handler
            .get_or_insert_default()
            .subscribe(event, handler);
    }
    
    /// Unsubscribe a handler from the `event`
    pub fn unsubscribe(&mut self, event: impl AsRef<str>) {
        if let Some(notification_handler) = &self.options.notification_handler {
            notification_handler.unsubscribe(event);
        } 
    }

    /// Retrieves task result. If the task is not completed yet, waits until it completes or cancels.
    #[cfg(feature = "tasks")]
    pub async fn get_task_result<T>(&mut self, id: impl Into<String>) -> Result<TaskPayload<T>, Error>
    where 
        T: DeserializeOwned
    {
        let params = GetTaskPayloadRequestParams { id: id.into() };
        self.command(crate::types::task::commands::RESULT, Some(params))
            .await?
            .into_result()
    }

    /// Retrieve task status 
    #[cfg(feature = "tasks")]
    pub async fn get_task(&mut self, id: impl Into<String>) -> Result<Task, Error> {
        let params = GetTaskRequestParams { id: id.into() };
        self.command(crate::types::task::commands::GET, Some(params))
            .await?
            .into_result()
    }
    
    /// Cancels a task that is currently running
    /// 
    /// # Panics
    /// If the client or server does not support cancelling tasks
    #[cfg(feature = "tasks")]
    pub async fn cancel_task(&mut self, id: impl Into<String>) -> Result<Task, Error> {
        assert!(
            self.is_client_support_cancelling_tasks(), 
            "Client does not support cancelling tasks.  You may configure it with `Client::with_options(|opt| opt.with_tasks(...))` method."
        );
        
        assert!(
            self.is_server_support_cancelling_tasks(), 
            "Server does not support cancelling tasks."
        );
        
        let params = CancelTaskRequestParams { id: id.into() };
        self.command(crate::types::task::commands::CANCEL, Some(params))
            .await?
            .into_result()
    }

    /// Retrieves a list of tasks
    /// 
    /// # Panics
    /// If the client or server does not support retrieving a task list
    #[cfg(feature = "tasks")]
    pub async fn list_tasks(&mut self, cursor: Option<Cursor>) -> Result<ListTasksResult, Error> {
        assert!(
            self.is_client_support_task_list(), 
            "Client does not support retrieving a task list.  You may configure it with `Client::with_options(|opt| opt.with_tasks(...))` method."
        );
        
        assert!(
            self.is_server_support_task_list(), 
            "Server does not support retrieving a task list."
        );
        
        let params = ListTasksRequestParams { cursor };
        self.command(crate::types::task::commands::LIST, Some(params))
            .await?
            .into_result()
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

    /// Returns whether the client has elicitation capabilities
    #[inline]
    fn is_elicitation_supported(&self) -> bool {
        self.options.elicitation_capability
            .as_ref()
            .is_some()
    }

    /// Returns whether the client has task augmentation capabilities
    #[inline]
    #[cfg(feature = "tasks")]
    fn is_client_supports_tasks(&self) -> bool {
        self.options.tasks_capability
            .as_ref()
            .is_some()
    }

    /// Returns whether the server has task augmentation capabilities
    #[inline]
    #[cfg(feature = "tasks")]
    fn is_server_supports_tasks(&self) -> bool {
        self.server_capabilities
            .as_ref()
            .is_some_and(|c| c.tasks.is_some())
    }

    /// Returns whether the client supports cancelling tasks
    #[inline]
    #[cfg(feature = "tasks")]
    fn is_client_support_cancelling_tasks(&self) -> bool {
        self.options.tasks_capability
            .as_ref()
            .is_some_and(|c| c.cancel.is_some())
    }

    /// Returns whether the server supports cancelling tasks
    #[inline]
    #[cfg(feature = "tasks")]
    fn is_server_support_cancelling_tasks(&self) -> bool {
        self.server_capabilities
            .as_ref()
            .and_then(|c| c.tasks.as_ref())
            .is_some_and(|c| c.cancel.is_some())
    }

    /// Returns whether the server supports retrieving a task list
    #[inline]
    #[cfg(feature = "tasks")]
    fn is_server_support_task_list(&self) -> bool {
        self.server_capabilities
            .as_ref()
            .and_then(|c| c.tasks.as_ref())
            .is_some_and(|c| c.list.is_some())
    }

    /// Returns whether the client supports retrieving a task list
    #[inline]
    #[cfg(feature = "tasks")]
    fn is_client_support_task_list(&self) -> bool {
        self.options.tasks_capability
            .as_ref()
            .is_some_and(|c| c.list.is_some())
    }

    /// Returns whether the server supports task-augmented tools
    #[inline]
    #[cfg(feature = "tasks")]
    fn is_server_support_call_tool_with_tasks(&self) -> bool {
        self.server_capabilities
            .as_ref()
            .and_then(|c| c.tasks.as_ref())
            .and_then(|c| c.requests.as_ref())
            .and_then(|r| r.tools.as_ref())
            .is_some_and(|t| t.call.is_some())
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
    
    #[inline(always)]
    #[cfg(feature = "tasks")]
    fn ensure_tasks_supported(&self) {
        assert!(
            self.is_client_supports_tasks(),
            "Client does not support task-augmented requests. You may configure it with `Client::with_options(|opt| opt.with_tasks(...))` method."
        );

        assert!(
            self.is_server_supports_tasks(),
            "Client does not support task-augmented requests."
        );
    }
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

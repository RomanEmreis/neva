//! Utilities for the MCP client

use std::collections::HashMap;
use options::McpOptions;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use handler::RequestHandler;
use crate::error::{Error, ErrorCode};
use crate::transport::Transport;
use crate::types::{
    ListToolsRequestParams, ListToolsResult, CallToolRequestParams, CallToolResponse,
    ListResourcesRequestParams, ListResourcesResult, ReadResourceRequestParams, ReadResourceResult,
    ListResourceTemplatesRequestParams, ListResourceTemplatesResult, Uri,
    ListPromptsRequestParams, ListPromptsResult, GetPromptRequestParams, GetPromptResult,
    ClientCapabilities, InitializeRequestParams, InitializeResult, ProgressToken, 
    Request, RequestId, Response, request::RequestParamsMeta,
    cursor::Cursor,
    notification::Notification,
};

mod handler;
pub mod options;

/// Represents an MCP client app 
pub struct Client {
    /// MCP client options
    options: McpOptions,
    
    /// A [`CancellationToken`] that cancels transport background processes
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
        let mut transport = self.options.transport();
        let token = transport.start();
        
        self.cancellation_token = Some(token);
        self.handler = Some(RequestHandler::new(transport, self.options.timeout));
        
        self.wait_for_shutdown_signal();
        self.init().await
    }

    /// Disconnects the MCP client from MCP server
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
        let cancelled = Notification::new("notifications/cancelled", None); 
        self.send_notification(cancelled).await?;

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
                experimental: None,
                sampling: None,
                roots: None
            })
        };

        let req = Request::new(None, "initialize", Some(params));
        let resp = self.send_request(req).await?;

        let init_result = resp.into_result::<InitializeResult>()?;
        assert_eq!(
            init_result.protocol_ver,
            self.options.protocol_ver(),
            "Server protocol version mismatch.");

        let initialized = Notification::new("notifications/initialized", None); 
        self.send_notification(initialized).await
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
    ///     // Fetch all or initial list of tools if MCP server provides pagination
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
            "tools/list",
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
    ///     // Fetch all or initial list of resources if MCP server provides pagination
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
            "resources/list", 
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
    ///     // Fetch all or initial list of resource templates if MCP server provides pagination
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
            "resources/templates/list",
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
    ///     // Fetch all or initial list of prompts if MCP server provides pagination
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
            "prompts/list",
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
    ///     let args = [("message", "Hello MCP!")];
    ///     let result = client.call_tool("echo", Some(args)).await?;
    ///     // Do something with the result
    ///
    ///     client.disconnect().await
    /// }
    /// ``` 
    pub async fn call_tool<I, T>(&mut self, name: &str, args: Option<I>) -> Result<CallToolResponse, Error>
    where
        I: IntoIterator<Item = (&'static str, T)>,
        T: Serialize,
    {
        let id = self.generate_id()?;
        let request = Request::new(
            Some(id.clone()),
            "tools/call",
            Some(CallToolRequestParams {
                meta: Some(RequestParamsMeta { 
                    progress_token: Some(ProgressToken::from(&id))
                }),
                name: name.into(),
                args: Self::create_args(args)
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
            "resources/read",
            Some(ReadResourceRequestParams {
                uri: uri.into(),
                meta: Some(RequestParamsMeta {
                    progress_token: Some(ProgressToken::from(&id))
                })
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
    ///     let prompt = client.get_prompt("complex_prompt", Some(args)).await?;
    ///     // Do something with the prompt
    ///
    ///     client.disconnect().await
    /// }
    /// ``` 
    pub async fn get_prompt<I, T>(&mut self, name: &str, args: Option<I>) -> Result<GetPromptResult, Error>
    where
        I: IntoIterator<Item = (&'static str, T)>,
        T: Serialize,
    {
        let id = self.generate_id()?;
        let request = Request::new(
            Some(id.clone()),
            "prompts/get",
            Some(GetPromptRequestParams {
                meta: Some(RequestParamsMeta {
                    progress_token: Some(ProgressToken::from(&id))
                }),
                name: name.into(),
                args: Self::create_args(args)
            })
        );

        self.send_request(request)
            .await?
            .into_result()
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
    async fn send_notification(&mut self, notification: Notification) -> Result<(), Error> {
        self.handler.as_mut()
            .ok_or_else(|| Error::new(ErrorCode::InternalError, "Connection closed"))?
            .send_notification(notification)
            .await
    }

    /// Generates a new [`RequestId`]
    #[inline]
    fn generate_id(&self) -> Result<RequestId, Error> {
        self.handler.as_ref()
            .ok_or_else(|| Error::new(ErrorCode::InternalError, "Connection closed"))
            .map(|h| h.next_id())
    }
    
    /// Creates arguments for tools and prompts from iterator
    #[inline]
    fn create_args<I, T>(args: Option<I>) -> Option<HashMap<String, serde_json::Value>>
    where
        I: IntoIterator<Item = (&'static str, T)>,
        T: Serialize,
    {
        args.map(|args| HashMap::from_iter(args
            .into_iter()
            .map(|(k, v)| (k.to_string(), serde_json::to_value(v).unwrap()))))
    }
    
    #[inline]
    fn wait_for_shutdown_signal(&mut self) {
        let Some(token) = self.cancellation_token.clone() else {
            return;
        };
        
        tokio::task::spawn(async move {
            match tokio::signal::ctrl_c().await {
                Ok(_) => (),
                #[cfg(feature = "tracing")]
                Err(err) => tracing::error!(
                    logger = "neva",
                    "Unable to listen for shutdown signal: {}", err),
                #[cfg(not(feature = "tracing"))]
                Err(_) => ()
            }
            token.cancel();
        });
    }
}

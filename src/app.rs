use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use crate::error::Error;
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
    ListResourcesRequestParams, ListResourcesResult,
    ReadResourceRequestParams, ReadResourceResult,
    SubscribeRequestParams, UnsubscribeRequestParams,
    ListPromptsRequestParams, ListPromptsResult, GetPromptRequestParams, GetPromptResult
};
use crate::types::resource::template::ResourceFunc;

pub mod options;
pub(crate) mod handler;

/// Represents an MCP server application
#[derive(Default)]
pub struct App {
    options: McpOptions,
    handlers: HashMap<String, RequestHandler<Response>>,
}

impl App {
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
    
    pub async fn run(mut self) {
        let mut transport = self.options.transport();
        let options = Arc::new(self.options);

        transport.start();
        
        while let Ok(req) = transport.recv().await {
            let req_id = req.id();
            let resp = match self.handlers.get(&req.method) {
                Some(handler) => handler.call(HandlerParams::Request(options.clone(), req)).await,
                None => Err(Error::new("unknown request"))
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
    
    pub fn map_handler<F, Args, R>(&mut self, name: &str, handler: F) -> &mut Self
    where 
        F: GenericHandler<Args, Output = R>,
        Args: FromHandlerParams + Send + Sync + 'static,
        R: IntoResponse + Send + 'static,
    {
        let handler = RequestFunc::new(handler);
        self.handlers.insert(name.into(), handler);
        self
    }
    
    pub fn map_tool<F, Args, R>(&mut self, name: &str, handler: F) -> &mut Self
    where
        F: ToolHandler<Args, Output = R>,
        Args: TryFrom<CallToolRequestParams, Error = Error> + Send + Sync + 'static,
        R: Into<CallToolResponse> + Send + 'static,
    {
        self.options.add_tool(Tool::new(name, handler));
        self
    }

    pub fn map_resource<F, Args, R>(&mut self, uri: &'static str, name: &str, handler: F) -> &mut Self
    where
        F: GenericHandler<Args, Output = R>,
        Args: TryFrom<ReadResourceRequestParams, Error = Error> + Send + Sync + 'static,
        R: Into<ReadResourceResult> + Send + 'static,
    {
        let handler = ResourceFunc::new(handler);
        let template = ResourceTemplate::new(uri, name);
        
        self.options.add_resource_template(template, handler);
        self
    }
    
    pub fn map_resources<F, R>(&mut self, handler: F) -> &mut Self
    where
        F: Fn(ListResourcesRequestParams) -> R + Clone + Send + Sync + 'static,
        R: Future + Send,
        R::Output: Into<ListResourcesResult>
    {
        self.map_handler(
            "resources/list", 
            move |params| {
                let handler = handler.clone();
                async move {
                    handler(params)
                        .await
                        .into()
                }
            }
        );
        self
    }

    async fn init(
        options: Arc<McpOptions>, 
        _params: InitializeRequestParams
    ) -> InitializeResult {
        InitializeResult::new(&options)
    }

    async fn completion(
        _: Arc<McpOptions>, 
        _params: CompleteRequestParams
    ) -> CompleteResult {
        // TODO: return default as it non-optional capability so far
        CompleteResult::default()
    }
    
    async fn tools(
        options: Arc<McpOptions>, 
        _params: ListToolsRequestParams
    ) -> ListToolsResult {
        options.tools()
    }

    async fn resources(
        options: Arc<McpOptions>,
        _params: ListResourcesRequestParams
    ) -> ListResourcesResult {
        options.resources()
    }

    async fn resource_templates(
        options: Arc<McpOptions>, 
        _params: ListResourceTemplatesRequestParams
    ) -> ListResourceTemplatesResult {
        options.resource_templates()
    }
    
    async fn prompts(
        options: Arc<McpOptions>, 
        _params: ListPromptsRequestParams
    ) -> ListPromptsResult {
        options.prompts()
    }
    
    async fn tool(
        options: Arc<McpOptions>, 
        params: CallToolRequestParams
    ) -> Result<CallToolResponse, Error> {
        match options.get_tool(&params.name) {
            Some(tool) => tool.call(params.into()).await,
            None => Err(Error::new("tool not found")),
        }
    }
    
    async fn resource(
        options: Arc<McpOptions>, 
        params: ReadResourceRequestParams) -> Result<ReadResourceResult, Error> {
        match options.read_resource(&params.uri) {
            Some(handler) => handler.call(params.into()).await,
            None => Err(Error::new("resource not found")),
        }
    }
    
    async fn prompt(
        _options: Arc<McpOptions>, 
        _params: GetPromptRequestParams
    ) -> GetPromptResult {
        // TODO: impl
        GetPromptResult::default()
    }

    async fn ping(_: Arc<McpOptions>) {}
    
    async fn notifications_init(_: Arc<McpOptions>) {}
    
    async fn notifications_cancel(_: Arc<McpOptions>) {}
    
    async fn resource_subscribe(
        _options: Arc<McpOptions>, 
        _params: SubscribeRequestParams
    ) -> Error {
        Error::new("resource_subscribe not implemented")
    }

    async fn resource_unsubscribe(
        _options: Arc<McpOptions>, 
        _params: UnsubscribeRequestParams
    ) -> Error {
        Error::new("resource_subscribe not implemented")
    }
}

#[cfg(test)]
mod tests {
    
}
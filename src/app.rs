use serde_json::from_value;
use crate::error::Error;
use crate::options::McpOptions;
use crate::transport::Transport;
use crate::types::{CallToolRequestParams, InitializeResult, IntoResponse, Request, Response, CompleteResult, Tool, ToolHandler, CallToolResponse};

pub mod options;

#[derive(Default)]
pub struct App {
    options: McpOptions
}

impl App {
    pub fn new() -> App {
        Self { options: McpOptions::default() }
    }
    
    pub async fn run(mut self) {
        let mut transport = self.options.transport();

        transport.start();
        
        while let Ok(req) = transport.recv().await {
            let resp = self.handle_request(req).await;
            match transport.send(resp).await { 
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
    
    pub fn map_tool<F, Args, R>(&mut self, name: &str, handler: F) -> &mut Self
    where
        F: ToolHandler<Args, Output = R>,
        Args: TryFrom<CallToolRequestParams, Error = Error> + Send + Sync + 'static,
        R: Into<CallToolResponse> + Send + 'static,
    {
        self.options.add_tool(Tool::new(name, handler));
        self
    }
    
    async fn handle_request(&self, req: Request) -> Response {
        match req.method.as_str() { 
            "ping" => Response::empty(req.into_id()),
            "initialize" => self.handle_initialize(req),
            "completion/complete" => self.handle_completion(req),
            "notifications/initialized" => Response::empty(req.into_id()),
            "notifications/cancelled" => Response::empty(req.into_id()),
            "tools/list" => self.handle_tools_list(req),
            "tools/call" => self.handle_tool_call(req).await,
            "resources/list" => self.handle_resources_list(req),
            "resources/read" => Response::error(req.into_id(), "not implemented"),
            "prompts/list" => self.handle_prompts_list(req),
            "prompts/get" => Response::error(req.into_id(), "not implemented"),
            _ => Response::error(req.into_id(), "unknown request")
        }
    }
    
    fn handle_initialize(&self, req: Request) -> Response {
        let result = InitializeResult::new(&self.options);
        result.into_response(req.id.unwrap_or_default())
    }

    fn handle_completion(&self, req: Request) -> Response {
        // TODO: return default as it non-optional capability so far
        let result = CompleteResult::default();
        result.into_response(req.into_id())
    }
    
    fn handle_tools_list(&self, req: Request) -> Response {
        self.options
            .tools()
            .into_response(req.into_id())
    }

    fn handle_resources_list(&self, req: Request) -> Response {
        self.options
            .resources()
            .into_response(req.into_id())
    }

    fn handle_prompts_list(&self, req: Request) -> Response {
        self.options
            .prompts()
            .into_response(req.into_id())
    }
    
    async fn handle_tool_call(&self, req: Request) -> Response {
        let req_id = req.id();

        let params = match req.params {
            None => return Response::error(req_id, "missing params"),
            Some(p) => match from_value::<CallToolRequestParams>(p) {
                Ok(mut params) => {
                    params.req_id = req_id;
                    params
                }
                Err(err) => return Response::error(req_id, &err.to_string()),
            },
        };

        match self.options.get_tool(&params.name) {
            Some(tool) => tool.call(params).await,
            None => Response::error(params.req_id, "tool not found"),
        }
    }
}

#[cfg(test)]
mod tests {
    
}
use serde_json::{from_value, json};
use crate::options::McpOptions;
use crate::transport::Transport;
use crate::types::{
    CallToolRequestParams,
    InitializeResult,
    IntoResponse, Request, Response,
    Tool, ToolHandler
};

pub mod options;

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
        Args: TryFrom<CallToolRequestParams> + Send + Sync + 'static,
        R: IntoResponse + Send + 'static,
        Args::Error: ToString + Send + Sync
    {
        self.options.add_tool(Tool::new(name, handler));
        self
    }
    
    async fn handle_request(&self, req: Request) -> Response {
        match req.method.as_str() { 
            "initialize" => self.handle_initialize(req),
            "tools/list" => self.handle_tools_list(req),
            "tools/call" => self.handle_tool_call(req).await,
            "ping" => Response::pong(req.id.unwrap_or_default()),
            _ => Response::error(req.id.unwrap_or_default(), "unknown request")
        }
    }
    
    fn handle_initialize(&self, req: Request) -> Response {
        let json = json!(InitializeResult::new());
        Response::success(req.id.unwrap_or_default(), json)
    }
    
    fn handle_tools_list(&self, req: Request) -> Response {
        let tools = json!({ "tools": self.options.tools() });
        Response::success(req.id.unwrap_or_default(), tools)
    }
    
    async fn handle_tool_call(&self, req: Request) -> Response {
        let req_id = req.id.unwrap_or_default();

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
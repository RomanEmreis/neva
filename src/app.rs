use crate::options::McpOptions;
use crate::transport::StdIo;
use crate::types::{CallToolRequestParams, InitializeRequestParams, InitializeResult, Request, Response, Tool};
use crate::types::tool::ToolHandler;

pub mod options;

pub struct App {
    options: McpOptions
}

impl App {
    pub fn new() -> App {
        Self { options: McpOptions::default() }
    }
    
    pub async fn run(self) {
        let mut transport = StdIo::start();
        while let Ok(req) = transport.recv().await {
            let resp = self.handle_request(req).await;
            transport.send(resp).await;
        }
    }
    
    pub fn map_tool<F, Args, R>(&mut self, name: &str, handler: F) -> &mut Self
    where
        F: ToolHandler<Args, Output = R>,
        Args: TryFrom<Request> + Send + Sync + 'static,
        R: Into<Response> + Send + 'static,
        Args::Error: ToString + Send + Sync
    {
        self.options.add_tool(Tool::new(name, handler));
        self
    }
    
    async fn handle_request(&self, req: Request) -> Response {
        match req.method.as_str() { 
            "initialize" => self.handle_initialize(
                req.params
                    .and_then(|params| serde_json::from_value(params).ok())),
            "tools/list" => self.handle_tools_list(),
            "tools/call" => self.handle_tool_call(req).await,
            _ => Response::error(4, "unknown request".into())
        }
    }
    
    fn handle_initialize(&self, _params: Option<InitializeRequestParams>) -> Response {
        let json = serde_json::json!(InitializeResult::new());
        Response::success(0, Some(json))
    }
    
    fn handle_tools_list(&self) -> Response {
        let tools = self.options
            .tools
            .iter().map(|(_, tool)| tool.clone())
            .collect::<Vec<_>>();
        
        let tools = serde_json::json!({ "tools": tools });
        Response::success(1, Some(tools))
    }
    
    async fn handle_tool_call(&self, req: Request) -> Response {
        let params = req.clone().params;
        let params = serde_json::from_value::<CallToolRequestParams>(params.unwrap()).unwrap();
        match self.options.tools.get(&params.name) { 
            Some(tool) => {
                tool.call(req).await
            },
            None => Response::error(3, "tool not found".into())
        }
    }
}

#[cfg(test)]
mod tests {
    
}
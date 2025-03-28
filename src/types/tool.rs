//! Represents an MCP tool

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use crate::types::{Request, Response};

/// Represents a tool that the server is capable of calling. Part of the [`ListToolsResponse`].
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Clone, Serialize)]
pub struct Tool {
    /// The name of the tool.
    pub name: String,
    
    /// A human-readable description of the tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    
    /// A JSON Schema object defining the expected parameters for the tool.
    /// 
    /// > Note: Needs to a valid JSON schema object that additionally is of type object.
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
    
    #[serde(skip)]
    handler: DynHandler
}

#[derive(Debug, Clone, Deserialize)]
pub struct CallToolRequestParams {
    /// Tool name.
    pub name: String,
    
    /// Optional arguments to pass to the tool.
    #[serde(rename = "arguments")]
    pub args: Option<HashMap<String, serde_json::Value>>,
}

pub struct CallToolResponse {
    
}

/// Represents a specific registered handler
pub(crate) type DynHandler = Arc<
    dyn Handler
    + Send
    + Sync
>;

/// Represents a Request -> Response handler
pub(crate) trait Handler {
    fn call(&self, req: Request) -> BoxFuture<Response>;
}

/// Describes a generic MCP Tool handler
pub trait ToolHandler<Args>: Clone + Send + Sync + 'static {
    type Output;
    type Future: Future<Output = Self::Output> + Send;

    fn call(&self, args: Args) -> Self::Future;
}

pub(crate) struct ToolFunc<F, R, Args>
where
    F: ToolHandler<Args, Output = R>,
    R: Into<Response>,
    Args: TryFrom<Request>
{
    func: F,
    _marker: std::marker::PhantomData<Args>,
}

impl<F, R ,Args> ToolFunc<F, R, Args>
where
    F: ToolHandler<Args, Output = R>,
    R: Into<Response>,
    Args: TryFrom<Request>
{
    /// Creates a new [`ToolFunc`] wrapped into [`Arc`]
    pub(crate) fn new(func: F) -> Arc<Self> {
        let func = Self { func, _marker: std::marker::PhantomData };
        Arc::new(func)
    }
}

impl<F, R ,Args> Handler for ToolFunc<F, R, Args>
where
    F: ToolHandler<Args, Output = R>,
    R: Into<Response>,
    Args: TryFrom<Request> + Send + Sync,
    Args::Error: ToString + Send + Sync
{
    #[inline]
    fn call(&self, req: Request) -> BoxFuture<Response> {
        Box::pin(async move {
            match Args::try_from(req) { 
                Err(err) => Response::error(10, err.to_string()),
                Ok(args) => self.func
                    .call(args)
                    .await
                    .into()
            }
        })
    }
}

impl Tool {
    /// Initializes a new [`Tool`]
    pub fn new<F, Args, R>(name: &str, handler: F) -> Self 
    where
        F: ToolHandler<Args, Output = R>,
        Args: TryFrom<Request> + Send + Sync + 'static,
        R: Into<Response> + Send + 'static,
        Args::Error: ToString + Send + Sync
    {
        let handler = ToolFunc::new(handler);
        let input_schema = serde_json::json!({ "type": "object", "properties": { "name": { "type": "string", "description": "some descr" } } }); 
        Self {
            name: name.into(),
            description: None,
            input_schema, 
            handler,
        }
    }
    
    pub fn with_description(mut self, description: &str) -> Self {
        self.description = Some(description.into());
        self
    }
    
    #[inline]
    pub async fn call(&self, req: Request) -> Response {
        self.handler.call(req).await
    }
}

macro_rules! impl_generic_tool_handler ({ $($param:ident)* } => {
    impl<Func, Fut: Send, $($param,)*> ToolHandler<($($param,)*)> for Func
    where
        Func: Fn($($param),*) -> Fut + Send + Sync + Clone + 'static,
        Fut: Future + 'static,
    {
        type Output = Fut::Output;
        type Future = Fut;

        #[inline]
        #[allow(non_snake_case)]
        fn call(&self, ($($param,)*): ($($param,)*)) -> Self::Future {
            (self)($($param,)*)
        }
    }
});

impl_generic_tool_handler! {}
impl_generic_tool_handler! { T1 }
impl_generic_tool_handler! { T1 T2 }
impl_generic_tool_handler! { T1 T2 T3 }
impl_generic_tool_handler! { T1 T2 T3 T4 }
impl_generic_tool_handler! { T1 T2 T3 T4 T5 }

#[cfg(test)]
mod tests {
    
}
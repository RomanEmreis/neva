//! Represents an MCP tool

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::error::Error;
use super::helpers::TypeCategory;
use crate::{
    app::handler::{Handler, HandlerParams, GenericHandler, RequestHandler},
    types::{RequestId, Response, IntoResponse}
};

pub use call_tool_response::CallToolResponse;
use crate::app::handler::FromHandlerParams;
use crate::types::{Request, request::FromRequest};

mod from_request;
pub mod call_tool_response;

/// Represents a tool that the server is capable of calling. Part of the [`ListToolsResponse`].
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Clone, Serialize)]
pub struct Tool {
    /// The name of the tool.
    pub name: String,
    
    /// A human-readable description of the tool.
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,
    
    /// A JSON Schema object defining the expected parameters for the tool.
    /// 
    /// > Note: Needs to a valid JSON schema object that additionally is of type object.
    #[serde(rename = "inputSchema")]
    pub input_schema: InputSchema,
    
    /// A tool call handler
    #[serde(skip)]
    handler: RequestHandler<CallToolResponse>
}

/// Sent from the client to request a list of tools the server has.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Deserialize)]
pub struct ListToolsRequestParams {
    /// An opaque token representing the current pagination position.
    /// If provided, the server should return results starting after this cursor.
    pub cursor: Option<String>,
}

/// A response to a request to list the tools available on the server.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Default, Serialize)]
pub struct ListToolsResult {
    /// The server's response to a tools/list request from the client.
    pub tools: Vec<Tool>
}

/// Used by the client to invoke a tool provided by the server.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Debug, Clone, Deserialize)]
pub struct CallToolRequestParams {
    /// Tool name.
    pub name: String,
    
    /// Optional arguments to pass to the tool.
    #[serde(rename = "arguments")]
    pub args: Option<HashMap<String, Value>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InputSchema {
    /// Schema object type
    /// 
    /// > Note: always "object"
    #[serde(rename = "type")]
    pub r#type: String,
    
    /// A list of properties for command
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<HashMap<String, SchemaProperty>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SchemaProperty {
    /// Property type
    #[serde(rename = "type")]
    pub r#type: String,

    /// A Human-readable description of a property
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,
}

impl IntoResponse for ListToolsResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        Response::success(req_id, serde_json::to_value(self).unwrap())
    }
}

impl From<Vec<Tool>> for ListToolsResult {
    #[inline]
    fn from(tools: Vec<Tool>) -> Self {
        Self { tools }
    }
}

impl ListToolsResult {
    /// Create a new [`ListToolsResult`]
    #[inline]
    pub fn new() -> Self {
        Default::default()
    }
}

impl InputSchema {
    /// Creates a new [`InputSchema`] object
    #[inline]
    pub(crate) fn new(props: Option<HashMap<String, SchemaProperty>>) -> Self {
        Self { r#type: "object".into(), properties: props }
    }
}

impl SchemaProperty {
    /// Creates a new [`SchemaProperty`] for a `T`
    #[inline]
    pub(crate) fn new<T: TypeCategory>() -> Self {
        Self { 
            r#type: T::category().into(),
            descr: None
        }
    }
}

impl FromHandlerParams for CallToolRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

impl FromHandlerParams for ListToolsRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

/// Describes a generic MCP Tool handler
pub trait ToolHandler<Args>: GenericHandler<Args> {
    #[inline]
    fn args() -> Option<HashMap<String, SchemaProperty>> {
        None
    }
}

pub(crate) struct ToolFunc<F, R, Args>
where
    F: ToolHandler<Args, Output = R>,
    R: Into<CallToolResponse>,
    Args: TryFrom<CallToolRequestParams, Error = Error>
{
    func: F,
    _marker: std::marker::PhantomData<Args>,
}

impl<F, R ,Args> ToolFunc<F, R, Args>
where
    F: ToolHandler<Args, Output = R>,
    R: Into<CallToolResponse>,
    Args: TryFrom<CallToolRequestParams, Error = Error>
{
    /// Creates a new [`ToolFunc`] wrapped into [`Arc`]
    pub(crate) fn new(func: F) -> Arc<Self> {
        let func = Self { func, _marker: std::marker::PhantomData };
        Arc::new(func)
    }
}

impl<F, R ,Args> Handler<CallToolResponse> for ToolFunc<F, R, Args>
where
    F: ToolHandler<Args, Output = R>,
    R: Into<CallToolResponse>,
    Args: TryFrom<CallToolRequestParams, Error = Error> + Send + Sync,
{
    #[inline]
    fn call(&self, params: HandlerParams) -> BoxFuture<Result<CallToolResponse, Error>> {
        let HandlerParams::Tool(params) = params else { 
            unreachable!()
        };
        Box::pin(async move {
            let args = Args::try_from(params)?;
            Ok(self.func
                .call(args)
                .await
                .into())
        })
    }
}

impl Tool {
    /// Initializes a new [`Tool`]
    pub fn new<F, Args, R>(name: &str, handler: F) -> Self 
    where
        F: ToolHandler<Args, Output = R>,
        R: Into<CallToolResponse> + Send + 'static,
        Args: TryFrom<CallToolRequestParams, Error = Error> + Send + Sync + 'static,
    {
        let handler = ToolFunc::new(handler);
        let input_schema = InputSchema::new(F::args());
        Self {
            name: name.into(),
            descr: None,
            input_schema, 
            handler,
        }
    }
    
    /// Sets a description for a tool
    pub fn with_description(mut self, description: &str) -> Self {
        self.descr = Some(description.into());
        self
    }
    
    /// Invoke a tool
    #[inline]
    pub(crate) async fn call(&self, params: HandlerParams) -> Result<CallToolResponse, Error> {
        self.handler.call(params).await
    }
}

macro_rules! impl_generic_tool_handler ({ $($param:ident)* } => {
    impl<Func, Fut: Send, $($param: TypeCategory,)*> ToolHandler<($($param,)*)> for Func
    where
        Func: Fn($($param),*) -> Fut + Send + Sync + Clone + 'static,
        Fut: Future + 'static,
    {
        #[inline]
        #[allow(unused_mut)]
        fn args() -> Option<HashMap<String, SchemaProperty>> {
            let mut args = HashMap::new();
            $(
            args.insert(
                std::any::type_name::<$param>().to_string(),
                SchemaProperty::new::<$param>()
            );
            )*
            if args.len() == 0 { 
                None
            } else {
                Some(args)
            }
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
    use super::*;
    
    #[tokio::test]
    async fn it_creates_and_calls_tool() {
        let tool = Tool::new("sum", |a: i32, b: i32| async move { a + b });
        
        let params = CallToolRequestParams {
            name: "sum".into(),
            args: Some(HashMap::from([
                ("a".into(), serde_json::to_value(5).unwrap()),
                ("b".into(), serde_json::to_value(2).unwrap()),
            ])),
        };
        
        let resp = tool.call(params.into()).await.unwrap();
        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"result":7}}"#);
    }
}
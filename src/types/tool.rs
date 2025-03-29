//! Represents an MCP tool

mod from_request;

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use super::helpers::TypeCategory;
use crate::types::{RequestId, Response, IntoResponse};

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
    
    #[serde(skip)]
    handler: DynHandler
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
    pub args: Option<HashMap<String, serde_json::Value>>,
    
    /// A Call tool request id
    #[serde(skip)]
    pub(crate) req_id: RequestId
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

/// Represents a specific registered handler
pub(crate) type DynHandler = Arc<
    dyn Handler
    + Send
    + Sync
>;

/// Represents a Request -> Response handler
pub(crate) trait Handler {
    fn call(&self, params: CallToolRequestParams) -> BoxFuture<Response>;
}

/// Describes a generic MCP Tool handler
pub trait ToolHandler<Args>: Clone + Send + Sync + 'static {
    type Output;
    type Future: Future<Output = Self::Output> + Send;

    fn call(&self, args: Args) -> Self::Future;
    
    #[inline]
    fn args() -> Option<HashMap<String, SchemaProperty>> {
        None
    }
}

pub(crate) struct ToolFunc<F, R, Args>
where
    F: ToolHandler<Args, Output = R>,
    R: IntoResponse,
    Args: TryFrom<CallToolRequestParams>
{
    func: F,
    _marker: std::marker::PhantomData<Args>,
}

impl<F, R ,Args> ToolFunc<F, R, Args>
where
    F: ToolHandler<Args, Output = R>,
    R: IntoResponse,
    Args: TryFrom<CallToolRequestParams>
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
    R: IntoResponse,
    Args: TryFrom<CallToolRequestParams> + Send + Sync,
    Args::Error: ToString + Send + Sync
{
    #[inline]
    fn call(&self, params: CallToolRequestParams) -> BoxFuture<Response> {
        Box::pin(async move {
            let id = params.req_id.clone();
            match Args::try_from(params) { 
                Err(err) => Response::error(id, &err.to_string()),
                Ok(args) => self.func
                    .call(args)
                    .await
                    .into_response(id)
            }
        })
    }
}

impl Tool {
    /// Initializes a new [`Tool`]
    pub fn new<F, Args, R>(name: &str, handler: F) -> Self 
    where
        F: ToolHandler<Args, Output = R>,
        Args: TryFrom<CallToolRequestParams> + Send + Sync + 'static,
        R: IntoResponse + Send + 'static,
        Args::Error: ToString + Send + Sync
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
    pub(crate) async fn call(&self, params: CallToolRequestParams) -> Response {
        self.handler.call(params).await
    }
}

macro_rules! impl_generic_tool_handler ({ $($param:ident)* } => {
    impl<Func, Fut: Send, $($param: TypeCategory,)*> ToolHandler<($($param,)*)> for Func
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
            req_id: RequestId::default(),
            args: Some(HashMap::from([
                ("a".into(), serde_json::to_value(5).unwrap()),
                ("b".into(), serde_json::to_value(2).unwrap()),
            ])),
        };
        
        let resp = tool.call(params).await;
        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"result":7}}"#);
    }
}
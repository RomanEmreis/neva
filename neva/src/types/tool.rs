//! Represents an MCP tool

use std::collections::HashMap;
#[cfg(feature = "server")]
use std::{future::Future, sync::Arc};
#[cfg(feature = "server")]
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(feature = "server")]
use crate::error::{Error, ErrorCode};
#[cfg(feature = "server")]
use super::helpers::TypeCategory;
use crate::types::{
    PropertyType,
    request::RequestParamsMeta,
    Cursor
};
#[cfg(feature = "server")]
use crate::types::{IntoResponse, Page, RequestId, Response};

#[cfg(feature = "server")]
use crate::types::{FromRequest, Request};

#[cfg(feature = "server")]
use crate::{
    Context,
    app::handler::{
        FromHandlerParams,
        Handler,
        HandlerParams,
        GenericHandler,
        RequestHandler,
    }
};

pub use call_tool_response::CallToolResponse;

#[cfg(feature = "server")]
mod from_request;
pub mod call_tool_response;

/// List of commands for Tools
pub mod commands {
    pub const LIST: &str = "tools/list";
    pub const LIST_CHANGED: &str = "notifications/tools/list_changed";
    pub const CALL: &str = "tools/call";
}

/// Represents a tool that the server is capable of calling. Part of the [`ListToolsResult`].
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Clone, Serialize, Deserialize)]
pub struct Tool {
    /// The name of the tool.
    pub name: String,
    
    /// A human-readable description of the tool.
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,
    
    /// A JSON Schema object defining the expected parameters for the tool.
    /// 
    /// > Note: Needs to a valid JSON schema object that additionally is of a type object.
    #[serde(rename = "inputSchema")]
    pub input_schema: InputSchema,
    
    /// A tool call handler
    #[serde(skip)]
    #[cfg(feature = "server")]
    handler: Option<RequestHandler<CallToolResponse>>,

    #[serde(skip)]
    #[cfg(feature = "http-server")]
    roles: Option<Vec<String>>,

    #[serde(skip)]
    #[cfg(feature = "http-server")]
    permissions: Option<Vec<String>>,
}

/// Sent from the client to request a list of tools the server has.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Serialize, Deserialize)]
pub struct ListToolsRequestParams {
    /// An opaque token representing the current pagination position.
    /// If provided, the server should return results starting after this cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
}

/// A response to a request to list the tools available on the server.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Serialize, Deserialize)]
pub struct ListToolsResult {
    /// The server's response to a tools/list request from the client.
    pub tools: Vec<Tool>,
    
    /// An opaque token representing the pagination position after the last returned result.
    ///
    /// When a paginated result has more data available, the `next_cursor`
    /// field will contain `Some` token that can be used in subsequent requests
    /// to fetch the next page. When there are no more results to return, the `next_cursor` field
    /// will be `None`.
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// Used by the client to invoke a tool provided by the server.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolRequestParams {
    /// Tool name.
    pub name: String,
    
    /// Optional arguments to pass to the tool.
    #[serde(rename = "arguments")]
    pub args: Option<HashMap<String, Value>>,

    /// Metadata related to the request that provides additional protocol-level information.
    /// 
    /// > **Note:** This can include progress tracking tokens and other protocol-specific properties
    /// > that are not part of the primary request parameters.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<RequestParamsMeta>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InputSchema {
    /// Schema object type
    /// 
    /// > Note: always "object"
    #[serde(rename = "type", default)]
    pub r#type: PropertyType,
    
    /// A list of properties for command
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<HashMap<String, SchemaProperty>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SchemaProperty {
    /// Property type
    #[serde(rename = "type", default)]
    pub r#type: PropertyType,

    /// A Human-readable description of a property
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,
}

#[cfg(feature = "server")]
impl IntoResponse for ListToolsResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        Response::success(req_id, serde_json::to_value(self).unwrap())
    }
}

#[cfg(feature = "server")]
impl From<Vec<Tool>> for ListToolsResult {
    #[inline]
    fn from(tools: Vec<Tool>) -> Self {
        Self {
            next_cursor: None,
            tools
        }
    }
}

#[cfg(feature = "server")]
impl From<Page<'_, Tool>> for ListToolsResult {
    #[inline]
    fn from(page: Page<Tool>) -> Self {
        Self {
            next_cursor: page.next_cursor,
            tools: page.items.to_vec()
        }
    }
}

#[cfg(feature = "server")]
impl ListToolsResult {
    /// Create a new [`ListToolsResult`]
    #[inline]
    pub fn new() -> Self {
        Default::default()
    }
}

impl Default for InputSchema {
    #[inline]
    fn default() -> Self {
        Self { 
            r#type: PropertyType::Object, 
            properties: Some(HashMap::new())
        }
    }
}

#[cfg(feature = "server")]
impl InputSchema {
    /// Creates a new [`InputSchema`] object
    #[inline]
    pub(crate) fn new(props: Option<HashMap<String, SchemaProperty>>) -> Self {
        Self { r#type: PropertyType::Object, properties: props }
    }
    
    /// Deserializes a new [`InputSchema`] from a JSON string
    #[inline]
    pub fn from_json_str(json: &str) -> Self {
        serde_json::from_str(json)
            .expect("InputSchema: Incorrect JSON string provided")
    }
    
    /// Adds a new property into the schema. 
    /// If a property with this name already exists, it overwrites it
    pub fn add_property<T: Into<PropertyType>>(
        mut self, 
        name: &str, 
        descr: &str, 
        property_type: T
    ) -> Self {
        self.properties
            .get_or_insert_with(HashMap::new)
            .insert(name.into(), SchemaProperty { 
                r#type: property_type.into(), 
                descr: Some(descr.into())
            });
        self
    }
}

#[cfg(feature = "server")]
impl SchemaProperty {
    /// Creates a new [`SchemaProperty`] for a `T`
    #[inline]
    pub(crate) fn new<T: TypeCategory>() -> Self {
        Self { 
            r#type: T::category(),
            descr: None
        }
    }
}

#[cfg(feature = "server")]
impl FromHandlerParams for CallToolRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

#[cfg(feature = "server")]
impl FromHandlerParams for ListToolsRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

/// Describes a generic MCP Tool handler
#[cfg(feature = "server")]
pub trait ToolHandler<Args>: GenericHandler<Args> {
    #[inline]
    fn args() -> Option<HashMap<String, SchemaProperty>> {
        None
    }
}

#[cfg(feature = "server")]
pub(crate) struct ToolFunc<F, R, Args>
where
    F: ToolHandler<Args, Output = R>,
    R: Into<CallToolResponse>,
    Args: TryFrom<CallToolRequestParams, Error = Error>
{
    func: F,
    _marker: std::marker::PhantomData<Args>,
}

#[cfg(feature = "server")]
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

#[cfg(feature = "server")]
impl<F, R ,Args> Handler<CallToolResponse> for ToolFunc<F, R, Args>
where
    F: ToolHandler<Args, Output = R>,
    R: Into<CallToolResponse>,
    Args: TryFrom<CallToolRequestParams, Error = Error> + Send + Sync,
{
    #[inline]
    fn call(&self, params: HandlerParams) -> BoxFuture<'_, Result<CallToolResponse, Error>> {
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

#[cfg(feature = "server")]
impl CallToolRequestParams {
    /// Includes [`Context`] into request metadata. If metadata is `None` it creates a new.
    pub(crate) fn with_context(mut self, ctx: Context) -> Self {
        self.meta.get_or_insert_default().context = Some(ctx);
        self
    }
}

#[cfg(feature = "server")]
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
            handler: Some(handler),
            #[cfg(feature = "http-server")]
            roles: None,
            #[cfg(feature = "http-server")]
            permissions: None,
        }
    }
    
    /// Sets a description for a tool
    pub fn with_description(&mut self, description: &str) -> &mut Self {
        self.descr = Some(description.into());
        self
    }
    
    /// Sets an [`InputSchema`] for the tool. 
    /// 
    /// > **Note:** Automatically generated schema will be overwritten
    pub fn with_schema<F>(&mut self, config: F) -> &mut Self
    where 
        F: FnOnce(InputSchema) -> InputSchema
    {
        self.input_schema = config(Default::default());
        self
    }
    
    /// Sets a list of roles that are allowed to invoke the tool
    #[cfg(all(feature = "server", feature = "http-server"))]
    pub fn with_roles<T, I>(&mut self, roles: T) -> &mut Self
    where 
        T: IntoIterator<Item = I>,
        I: Into<String>
    {
        self.roles = Some(roles
            .into_iter()
            .map(Into::into)
            .collect());
        self
    }
    
    /// Sets a list of permissions that are allowed to invoke the tool
    #[cfg(all(feature = "server", feature = "http-server"))]
    pub fn with_permissions<T, I>(&mut self, permissions: T) -> &mut Self
    where
        T: IntoIterator<Item = I>,
        I: Into<String>
    {
        self.permissions = Some(permissions
            .into_iter()
            .map(Into::into)
            .collect());
        self
    }
    
    /// Validates tool params
    #[inline]
    #[cfg(feature = "http-server")]
    pub(crate) fn validate(&self, params: &HandlerParams) -> Result<(), Error> {
        use volga::auth::AuthClaims;

        let HandlerParams::Tool(tool_params) = params else {
            return Err(ErrorCode::InvalidParams.into());
        };

        if self.roles.is_none() && self.permissions.is_none() {
            return Ok(());
        }

        const ERR_NO_CLAIMS: &str = "Claims are not provided";
        const ERR_UNAUTHORIZED: &str = "Subject is not authorized to invoke this tool";

        let claims = tool_params
            .meta
            .as_ref()
            .and_then(|m| m.context.as_ref())
            .and_then(|ctx| ctx.claims.as_ref());

        if claims.is_none() {
            return Err(Error::new(ErrorCode::InvalidParams, ERR_NO_CLAIMS));
        }

        let contains_any = |have: Option<&[String]>, required: &[String]| {
            have.is_some_and(|vals| vals.iter().any(|v| required.contains(v)))
        };

        let contains = |have: Option<&str>, required: &[String]| {
            have.is_some_and(|val| required.iter().any(|r| r == val))
        };

        // Roles check
        if let Some(required_roles) = &self.roles {
            if !contains(claims.and_then(|c| c.role()), required_roles) &&
                !contains_any(claims.and_then(|c| c.roles()), required_roles) {
                return Err(Error::new(ErrorCode::InvalidParams, ERR_UNAUTHORIZED));
            }
        }

        // Permissions check
        if let Some(required_permissions) = &self.permissions {
            if !contains_any(claims.and_then(|c| c.permissions()), required_permissions) {
                return Err(Error::new(ErrorCode::InvalidParams, ERR_UNAUTHORIZED));
            }
        }

        Ok(())
    }
    
    /// Invoke a tool
    #[inline]
    pub(crate) async fn call(&self, params: HandlerParams) -> Result<CallToolResponse, Error> {
        #[cfg(feature = "http-server")]
        self.validate(&params)?;
        match self.handler { 
            Some(ref handler) => handler.call(params).await,
            None => Err(Error::new(ErrorCode::InternalError, "Tool handler not specified"))
        }
    }
}

macro_rules! impl_generic_tool_handler ({ $($param:ident)* } => {
    #[cfg(feature = "server")]
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
            {
                let prop = SchemaProperty::new::<$param>();
                if prop.r#type != PropertyType::None {
                    args.insert(
                        prop.r#type.to_string(),
                        prop
                    );
                }
            };
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
#[cfg(feature = "server")]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn it_creates_and_calls_tool() {
        let tool = Tool::new("sum", |a: i32, b: i32| async move { a + b });
        
        let params = CallToolRequestParams {
            name: "sum".into(),
            meta: None,
            args: Some(HashMap::from([
                ("a".into(), serde_json::to_value(5).unwrap()),
                ("b".into(), serde_json::to_value(2).unwrap()),
            ])),
        };
        
        let resp = tool.call(params.into()).await.unwrap();
        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[{"type":"text","text":"7","mimeType":"text/plain"}],"isError":false}"#);
    }
    
    #[test]
    fn it_deserializes_input_schema() {
        let json = r#"{ 
            "properties": {
                "name": { 
                    "type": "string",
                    "description": "A name to whom say hello"
                }
            }
        }"#;
        
        let schema: InputSchema = serde_json::from_str(json).unwrap();
        
        assert_eq!(schema.r#type, PropertyType::Object);
        assert!(schema.properties.is_some());
    }
}
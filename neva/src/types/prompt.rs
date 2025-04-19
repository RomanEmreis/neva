//! Represents an MCP prompt

use std::collections::HashMap;
use std::sync::Arc;
use std::future::Future;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use super::helpers::TypeCategory;
use crate::types::{Cursor, IntoResponse, Page, PropertyType, Request, RequestId, Response};
use crate::app::handler::{FromHandlerParams, HandlerParams, GenericHandler, Handler, RequestHandler};
use crate::error::Error;
use crate::types::request::{FromRequest, RequestParamsMeta};

pub use get_prompt_result::{GetPromptResult, PromptMessage};

mod from_request;
pub mod get_prompt_result;

/// A prompt or prompt template that the server offers.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Clone, Serialize)]
pub struct Prompt {
    /// The name of the prompt or prompt template.
    pub name: String,

    /// An optional description of what this prompt provides
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,

    /// A list of arguments to use for templating the prompt.
    #[serde(rename = "arguments", skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<PromptArgument>>,

    /// A get prompt handler
    #[serde(skip)]
    handler: RequestHandler<GetPromptResult>
}

/// Describes an argument that a prompt can accept.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Clone, Serialize, Deserialize)]
pub struct PromptArgument {
    /// The name of the argument.
    pub name: String,

    /// A human-readable description of the argument.
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,

    /// Whether this argument must be provided.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

/// Sent from the client to request a list of prompts and prompt templates the server has.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Deserialize)]
pub struct ListPromptsRequestParams {
    /// An opaque token representing the current pagination position.
    /// If provided, the server should return results starting after this cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
}

/// Used by the client to get a prompt provided by the server.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Deserialize)]
pub struct GetPromptRequestParams {
    /// The name of the prompt or prompt template.
    pub name: String,
    
    /// Arguments to use for templating the prompt.
    #[serde(rename = "arguments", skip_serializing_if = "Option::is_none")]
    pub args: Option<HashMap<String, Value>>,

    /// Metadata related to the request that provides additional protocol-level information.
    ///
    /// > **Note:** This can include progress tracking tokens and other protocol-specific properties
    /// > that are not part of the primary request parameters.
    #[serde(rename = "_meta")]
    pub meta: Option<RequestParamsMeta>,
}

/// The server's response to a prompts/list request from the client.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Default, Serialize)]
pub struct ListPromptsResult {
    /// A list of prompts or prompt templates that the server offers.
    pub prompts: Vec<Prompt>,

    /// An opaque token representing the pagination position after the last returned result.
    ///
    /// When a paginated result has more data available, the `next_cursor`
    /// field will contain `Some` token that can be used in subsequent requests
    /// to fetch the next page. When there are no more results to return, the `next_cursor` field
    /// will be `None`.
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

impl IntoResponse for ListPromptsResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        Response::success(req_id, serde_json::to_value(self).unwrap())
    }
}

impl From<Vec<Prompt>> for ListPromptsResult {
    #[inline]
    fn from(prompts: Vec<Prompt>) -> Self {
        Self {
            next_cursor: None,
            prompts
        }
    }
}

impl From<Page<'_, Prompt>> for ListPromptsResult {
    #[inline]
    fn from(page: Page<Prompt>) -> Self {
        Self {
            next_cursor: page.next_cursor,
            prompts: page.items.to_vec()
        }
    }
}

impl ListPromptsResult {
    /// Create a new [`ListPromptsResult`]
    #[inline]
    pub fn new() -> Self {
        Default::default()
    }
}

impl FromHandlerParams for ListPromptsRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

impl FromHandlerParams for GetPromptRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

impl From<&str> for PromptArgument {
    #[inline]
    fn from(name: &str) -> Self {
        Self {
            name: name.into(), 
            descr: None,
            required: Some(true)
        }
    }
}

impl From<Value> for PromptArgument {
    #[inline]
    fn from(json: Value) -> Self {
        serde_json::from_value(json)
            .expect("A correct PromptArgument value must be provided")
    }
}

impl From<String> for PromptArgument {
    #[inline]
    fn from(name: String) -> Self {
        Self {
            name,
            descr: None,
            required: Some(true),
        }
    }
}

impl From<(&str, &str)> for PromptArgument {
    #[inline]
    fn from((name, description): (&str, &str)) -> Self {
        Self::required(name, description)
    }
}

impl From<(&str, &str, bool)> for PromptArgument {
    #[inline]
    fn from((name, description, required): (&str, &str, bool)) -> Self {
        Self {
            name: name.into(),
            descr: Some(description.into()),
            required: Some(required),
        }
    }
}

/// Describes a generic get prompt handler
pub trait PromptHandler<Args>: GenericHandler<Args> {
    #[inline]
    fn args() -> Option<Vec<PromptArgument>> {
        None
    }
}

pub(crate) struct PromptFunc<F, R, Args>
where
    F: PromptHandler<Args, Output = R>,
    R: TryInto<GetPromptResult>,
    R::Error: Into<Error>,
    Args: TryFrom<GetPromptRequestParams, Error = Error>,
{
    func: F,
    _marker: std::marker::PhantomData<Args>,
}

impl<F, R, Args> PromptFunc<F, R, Args>
where
    F: PromptHandler<Args, Output = R>,
    R: TryInto<GetPromptResult>,
    R::Error: Into<Error>,
    Args: TryFrom<GetPromptRequestParams, Error = Error>
{
    /// Creates a new [`PromptFunc`] wrapped into [`Arc`]
    pub(crate) fn new(func: F) -> Arc<Self> {
        let func = Self { func, _marker: std::marker::PhantomData };
        Arc::new(func)
    }
}

impl<F, R, Args> Handler<GetPromptResult> for PromptFunc<F, R, Args>
where
    F: PromptHandler<Args, Output = R>,
    R: TryInto<GetPromptResult>,
    R::Error: Into<Error>,
    Args: TryFrom<GetPromptRequestParams, Error = Error> + Send + Sync
{
    #[inline]
    fn call(&self, params: HandlerParams) -> BoxFuture<Result<GetPromptResult, Error>> {
        let HandlerParams::Prompt(params) = params else {
            unreachable!()
        };
        Box::pin(async move {
            let args = Args::try_from(params)?;
            self.func
                .call(args)
                .await
                .try_into()
                .map_err(Into::into)
        })
    }
}

impl Prompt {
    /// Creates a new [`Prompt`]
    #[inline]
    pub fn new<F, R, Args>(name: &str, handler: F) -> Self
    where
        F: PromptHandler<Args, Output = R>,
        R: TryInto<GetPromptResult> + Send + 'static,
        R::Error: Into<Error>,
        Args: TryFrom<GetPromptRequestParams, Error = Error>  + Send + Sync + 'static,
    {
        let handler = PromptFunc::new(handler);
        let args = F::args();
        Self { 
            name: name.into(), 
            descr: None, 
            args,
            handler
        }
    }
    
    /// Sets a [`Prompt`] description
    pub fn with_description(&mut self, descr: &str) -> &mut Self {
        self.descr = Some(descr.into());
        self
    }
    
    /// Sets arguments for the [`Prompt`]
    pub fn with_args<T, A>(&mut self, args: T) -> &mut Self
    where
        T: IntoIterator<Item = A>,
        A: Into<PromptArgument>,
    {
        self.args = Some(args
            .into_iter()
            .map(Into::into)
            .collect());
        self
    }

    /// Get prompt result
    #[inline]
    pub(crate) async fn call(&self, params: HandlerParams) -> Result<GetPromptResult, Error> {
        self.handler.call(params).await
    }
}

pub struct PromptArguments;
impl PromptArguments {
    /// Deserializes a [`Vec`] of [`PromptArgument`] from JSON string
    #[inline]
    pub fn from_json_str(json: &str) -> Vec<Value> {
        serde_json::from_str(json)
            .expect("PromptArgument: Incorrect JSON string provided")
    }
}

impl PromptArgument {
    /// Creates a new [`PromptArgument`] 
    pub(crate) fn new<T>() -> Self {
        Self {
            name: std::any::type_name::<T>().into(),
            descr: None,
            required: Some(true),
        }
    }
    
    /// Creates a new required [`PromptArgument`]
    pub fn required(name: &str, descr: &str) -> Self {
        Self {
            name: name.into(),
            descr: Some(descr.into()),
            required: Some(true),
        }
    }

    /// Creates a new required [`PromptArgument`]
    pub fn optional(name: &str, descr: &str) -> Self {
        Self {
            name: name.into(),
            descr: Some(descr.into()),
            required: Some(false),
        }
    }
}

macro_rules! impl_generic_prompt_handler ({ $($param:ident)* } => {
    impl<Func, Fut: Send, $($param: TypeCategory,)*> PromptHandler<($($param,)*)> for Func
    where
        Func: Fn($($param),*) -> Fut + Send + Sync + Clone + 'static,
        Fut: Future + 'static,
    {
        #[inline]
        #[allow(unused_mut)]
        fn args() -> Option<Vec<PromptArgument>> {
            let mut args = Vec::new();
            $(
            {
                if $param::category() != PropertyType::None { 
                    args.push(PromptArgument::new::<$param>());
                } 
            }
            )*
            if args.len() == 0 { 
                None
            } else {
                Some(args)
            }
        }
    }
});

impl_generic_prompt_handler! {}
impl_generic_prompt_handler! { T1 }
impl_generic_prompt_handler! { T1 T2 }
impl_generic_prompt_handler! { T1 T2 T3 }
impl_generic_prompt_handler! { T1 T2 T3 T4 }
impl_generic_prompt_handler! { T1 T2 T3 T4 T5 }
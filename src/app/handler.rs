//! Handler utilities for resources, tools and prompts

use std::future::Future;
use std::sync::Arc;
use futures_util::future::BoxFuture;
use serde::de::DeserializeOwned;
use serde_json::json;
use crate::error::Error;
use crate::options::McpOptions;
use crate::types::{
    CallToolRequestParams, 
    ReadResourceRequestParams, 
    GetPromptRequestParams, 
    IntoResponse, 
    Request, 
    Response
};

/// Represents a specific registered handler
pub(crate) type RequestHandler<T> = Arc<
    dyn Handler<T>
    + Send
    + Sync
>;

pub(crate) enum HandlerParams {
    Request(Arc<McpOptions>, Request),
    Tool(CallToolRequestParams),
    Resource(ReadResourceRequestParams),
    Prompt(GetPromptRequestParams)
}

impl From<CallToolRequestParams> for HandlerParams {
    #[inline]
    fn from(params: CallToolRequestParams) -> Self {
        Self::Tool(params)
    }
}

impl From<ReadResourceRequestParams> for HandlerParams {
    #[inline]
    fn from(params: ReadResourceRequestParams) -> Self {
        Self::Resource(params)
    }
}

impl From<GetPromptRequestParams> for HandlerParams {
    #[inline]
    fn from(params: GetPromptRequestParams) -> Self {
        Self::Prompt(params)
    }
}

/// Represents a Request -> Response handler
pub(crate) trait Handler<T: IntoResponse> {
    fn call(&self, params: HandlerParams) -> BoxFuture<Result<T, Error>>;
}

pub trait FromRequest: Sized {
    fn from_request(request: Request) -> Result<Self, Error>;
}

pub trait GenericHandler<Args>: Clone + Send + Sync + 'static  {
    type Output;
    type Future: Future<Output = Self::Output> + Send;
    
    fn call(&self, args: Args) -> Self::Future;
}

pub(crate) struct RequestFunc<F, R, Args>
where 
    F: GenericHandler<(Arc<McpOptions>, Args), Output = R>,
    R: IntoResponse,
    Args: FromRequest,
{
    func: F,
    _marker: std::marker::PhantomData<Args>,    
}

impl<F, R, Args> RequestFunc<F, R, Args>
where
    F: GenericHandler<(Arc<McpOptions>, Args), Output = R>,
    R: IntoResponse,
    Args: FromRequest
{
    pub(crate) fn new(func: F) -> Arc<Self> {
        let func = Self { func, _marker: std::marker::PhantomData };
        Arc::new(func)
    }
}

impl<F, R, Args> Handler<Response> for RequestFunc<F, R, Args>
where
    F: GenericHandler<(Arc<McpOptions>, Args), Output = R>,
    R: IntoResponse,
    Args: FromRequest + Send + Sync
{
    #[inline]
    fn call(&self, params: HandlerParams) -> BoxFuture<Result<Response, Error>> {
        let HandlerParams::Request(options, req) = params else {
            unreachable!()
        };
        Box::pin(async move {
            let id = req.id();
            let args = Args::from_request(req)?;
            Ok(self.func
                .call((options, args))
                .await
                .into_response(id))
        })
    }
}

impl<T: DeserializeOwned> FromRequest for T {
    #[inline]
    fn from_request(req: Request) -> Result<Self, Error> {
        let params = req.params
            .unwrap_or_else(|| json!({}));
        let args = T::deserialize(params)?;
        Ok(args)
    }
}

macro_rules! impl_generic_handler ({ $($param:ident)* } => {
    impl<Func, Fut: Send, $($param,)*> GenericHandler<($($param,)*)> for Func
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

impl_generic_handler! {}
impl_generic_handler! { T1 }
impl_generic_handler! { T1 T2 }
impl_generic_handler! { T1 T2 T3 }
impl_generic_handler! { T1 T2 T3 T4 }
impl_generic_handler! { T1 T2 T3 T4 T5 }
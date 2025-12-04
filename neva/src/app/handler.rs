//! Handler utilities for resources, tools and prompts

use std::future::Future;
use std::sync::Arc;
use futures_util::future::BoxFuture;
use crate::error::{Error, ErrorCode};
use crate::app::options::RuntimeMcpOptions;
use crate::Context;
use crate::types::{
    ListResourcesRequestParams,
    CompleteRequestParams,
    CallToolRequestParams, 
    ReadResourceRequestParams,
    GetPromptRequestParams,
    IntoResponse, Response,
    Request, RequestId
};

/// Represents a specific registered handler
pub(crate) type RequestHandler<T> = Arc<
    dyn Handler<T>
    + Send
    + Sync
>;

#[derive(Debug)]
pub enum HandlerParams {
    Request(Context, Request),
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
    fn call(&self, params: HandlerParams) -> BoxFuture<'_, Result<T, Error>>;
}

/// Represents an extractor trait from handler parameters
pub trait FromHandlerParams: Sized {
    fn from_params(params: &HandlerParams) -> Result<Self, Error>;
}

/// Represents a generic handler
pub trait GenericHandler<Args>: Clone + Send + Sync + 'static  {
    /// Output type
    type Output;
    /// Output future
    type Future: Future<Output = Self::Output> + Send;
    
    fn call(&self, args: Args) -> Self::Future;
}

/// Represents a generic handler for list resources
pub trait ListResourcesHandler<Args>: Clone + Send + Sync + 'static  {
    /// Output type
    type Output;
    /// Output future
    type Future: Future<Output = Self::Output> + Send;

    /// Calls the list resources handler
    fn call(&self, params: ListResourcesRequestParams, args: Args) -> Self::Future;
}

/// Represents a generic completion handler.
pub trait CompletionHandler<Args>: Clone + Send + Sync + 'static  {
    /// Output type
    type Output;
    /// Output future
    type Future: Future<Output = Self::Output> + Send;

    /// Calls the completion handler
    fn call(&self, params: CompleteRequestParams, args: Args) -> Self::Future;
}

pub(crate) struct RequestFunc<F, R, Args>
where 
    F: GenericHandler<Args, Output = R>,
    R: IntoResponse,
    Args: FromHandlerParams,
{
    func: F,
    _marker: std::marker::PhantomData<Args>,    
}

impl<F, R, Args> RequestFunc<F, R, Args>
where
    F: GenericHandler<Args, Output = R>,
    R: IntoResponse,
    Args: FromHandlerParams
{
    pub(crate) fn new(func: F) -> Arc<Self> {
        let func = Self { func, _marker: std::marker::PhantomData };
        Arc::new(func)
    }
}

impl<F, R, Args> Handler<Response> for RequestFunc<F, R, Args>
where
    F: GenericHandler<Args, Output = R>,
    R: IntoResponse,
    Args: FromHandlerParams + Send + Sync
{
    #[inline]
    fn call(&self, params: HandlerParams) -> BoxFuture<'_, Result<Response, Error>> {
        Box::pin(async move {
            let id = RequestId::from_params(&params)?;
            let args = Args::from_params(&params)?;
            Ok(self.func
                .call(args)
                .await
                .into_response(id))
        })
    }
}

impl FromHandlerParams for () {
    fn from_params(_: &HandlerParams) -> Result<Self, Error> {
        Ok(())
    }
}

impl FromHandlerParams for RequestId {
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Ok(req.id())
    }
}

impl FromHandlerParams for Context {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        match params {
            HandlerParams::Request(context, _) => Ok(context.clone()),
            _ => Err(Error::new(ErrorCode::InternalError, "invalid handler parameters"))
        }
    }
}

impl FromHandlerParams for RuntimeMcpOptions {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        match params {
            HandlerParams::Request(ctx, _) => Ok(ctx.options.clone()),
            _ => Err(Error::new(ErrorCode::InternalError, "invalid handler parameters"))
        }
    }
}

impl FromHandlerParams for Request {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        match params {
            HandlerParams::Request(_, req) => Ok(req.clone()),
            _ => Err(Error::new(ErrorCode::InternalError, "invalid handler parameters"))
        }
    }
}

macro_rules! impl_from_handler_params {
    ($($T: ident),*) => {
        impl<$($T: FromHandlerParams),+> FromHandlerParams for ($($T,)+) {
            #[inline]
            fn from_params(params: &HandlerParams) -> Result<Self, Error> {
                let args = ($(
                    $T::from_params(params)?,
                )*);
                Ok(args)
            }
        }
    };
}

impl_from_handler_params! { T1 }
impl_from_handler_params! { T1, T2 }
impl_from_handler_params! { T1, T2, T3 }
impl_from_handler_params! { T1, T2, T3, T4 }
impl_from_handler_params! { T1, T2, T3, T4, T5 }

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
    impl<Func, Fut: Send, $($param,)*> ListResourcesHandler<($($param,)*)> for Func
    where
        Func: Fn(ListResourcesRequestParams, $($param),*) -> Fut + Send + Sync + Clone + 'static,
        Fut: Future + 'static,
    {
        type Output = Fut::Output;
        type Future = Fut;

        #[inline]
        #[allow(non_snake_case)]
        fn call(&self, params: ListResourcesRequestParams, ($($param,)*): ($($param,)*)) -> Self::Future {
            (self)(params, $($param,)*)
        }
    }
    impl<Func, Fut: Send, $($param,)*> CompletionHandler<($($param,)*)> for Func
    where
        Func: Fn(CompleteRequestParams, $($param),*) -> Fut + Send + Sync + Clone + 'static,
        Fut: Future + 'static,
    {
        type Output = Fut::Output;
        type Future = Fut;

        #[inline]
        #[allow(non_snake_case)]
        fn call(&self, params: CompleteRequestParams, ($($param,)*): ($($param,)*)) -> Self::Future {
            (self)(params, $($param,)*)
        }
    }
});

impl_generic_handler! {}
impl_generic_handler! { T1 }
impl_generic_handler! { T1 T2 }
impl_generic_handler! { T1 T2 T3 }
impl_generic_handler! { T1 T2 T3 T4 }
impl_generic_handler! { T1 T2 T3 T4 T5 }

#[cfg(test)]
mod tests {
    
}
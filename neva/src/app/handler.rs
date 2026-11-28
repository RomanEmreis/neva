//! Handler utilities for resources, tools and prompts

use crate::Context;
use crate::app::options::RuntimeMcpOptions;
use crate::error::{Error, ErrorCode};
use crate::shared::BoxFuture;
use crate::shared::{BlockingCall, BlockingFn, marker};
use crate::types::{
    ArgNames, CallToolRequestParams, CompleteRequestParams, CompleteResult, GetPromptRequestParams,
    IntoResponse, ListResourcesRequestParams, ListResourcesResult, ReadResourceRequestParams,
    Request, RequestId, Response,
};
use std::future::Future;
use std::sync::Arc;

/// Represents a specific registered handler
pub(crate) type RequestHandler<T> = Arc<dyn Handler<T> + Send + Sync>;

/// Parameters handed to a registered handler.
///
/// The `Tool` and `Prompt` variants carry the [`ArgNames`] of the primitive
/// they were dispatched to, because extraction reads the request's
/// `arguments` map by name and only the registered [`crate::types::Tool`] /
/// [`crate::types::prompt::Prompt`] knows what its handler's arguments are
/// called.
///
/// The `Request` variant shares its request rather than handing it over: the
/// dispatcher keeps it for the rest of the round, since MRTR binds the state
/// it mints to the request's params, and reads them, if at all, once the
/// handler is done.
#[derive(Debug)]
pub enum HandlerParams {
    Request(Context, Arc<Request>),
    Tool(CallToolRequestParams, ArgNames),
    Resource(ReadResourceRequestParams),
    Prompt(GetPromptRequestParams, ArgNames),
}

impl From<ReadResourceRequestParams> for HandlerParams {
    #[inline]
    fn from(params: ReadResourceRequestParams) -> Self {
        Self::Resource(params)
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

/// The params of an MCP method, which a handler takes by declaring them.
///
/// They are read from the request in place -- `serde_json` deserializes from
/// a borrowed value as well as from an owned one -- and absent params read as
/// an empty object, which a params type of optional fields takes for its
/// defaults, as [`FromRequest`](crate::types::FromRequest) does.
pub(crate) trait RequestParams: serde::de::DeserializeOwned {}

impl<T: RequestParams> FromHandlerParams for T {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        match params {
            HandlerParams::Request(_, req) => match &req.params {
                Some(params) => T::deserialize(params),
                None => T::deserialize(serde_json::Value::Object(Default::default())),
            }
            .map_err(|err| Error::new(ErrorCode::InvalidParams, err)),
            _ => Err(Error::new(
                ErrorCode::InternalError,
                "invalid handler parameters",
            )),
        }
    }
}

/// Represents a generic request handler, as registered by
/// [`App::map_handler`](crate::App::map_handler).
///
/// Implemented for every function whose parameters are extractable and whose
/// return type is an [`IntoResponse`], in both shapes a handler can take: an
/// **asynchronous** one returning a future of such a value ([`marker::Async`],
/// the default) and a **synchronous** one returning the value itself
/// ([`marker::Immediate`]).
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid request handler",
    label = "not a request handler",
    note = "a request handler is a function of extractable arguments returning either a value \
            that implements `IntoResponse` or a future of one"
)]
pub trait GenericHandler<Args, M = marker::Async>: HandlerFn<Args, M> {}

/// The call mechanics every handler trait shares: what a handler returns and
/// how it is invoked.
///
/// `M` is one of the [`marker`] types and records which shape the implementing
/// function has. Both markers are implemented for every supported arity, so
/// this trait says nothing about whether a function is a *valid* handler: the
/// [`marker::Immediate`] impl accepts any return type at all.
///
/// That is deliberate, and it is also why **no public method may be bound on
/// this trait directly**. With both markers implemented, `M` would be left
/// unconstrained and inference fails with E0283 ("multiple `impl`s
/// satisfying ..."). Bound on a per-primitive trait instead --
/// [`ToolHandler`](crate::types::ToolHandler) and its siblings. Each of those
/// carries the conversion its registration point requires
/// (`Into<CallToolResponse>` and so on) *on the impl*, which is what prunes the
/// candidate set and pins `M`: a future does not convert into a tool response,
/// and a tool response is not a future.
pub trait HandlerFn<Args, M>: Clone + Send + Sync + 'static {
    /// Output type
    type Output;
    /// Output future
    type Future: Future<Output = Self::Output> + Send;

    /// Calls the handler
    fn call(&self, args: Args) -> Self::Future;
}

/// Represents a generic handler for list resources, as registered by
/// [`App::map_resources`](crate::App::map_resources).
///
/// Like every handler trait it comes in both shapes -- [`marker::Async`] (the
/// default) and [`marker::Immediate`] -- and `M` is inferred from the
/// handler's signature. It carries its own call mechanics rather than
/// borrowing [`HandlerFn`]'s: a list handler is passed the request parameters
/// alongside its extracted arguments.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid list resources handler",
    label = "not a list resources handler",
    note = "a list resources handler takes `ListResourcesRequestParams` followed by extractable \
            arguments and returns either a value that converts into `ListResourcesResult` or a \
            future of one"
)]
pub trait ListResourcesHandler<Args, M = marker::Async>: Clone + Send + Sync + 'static {
    /// Output type
    type Output;
    /// Output future
    type Future: Future<Output = Self::Output> + Send;

    /// Calls the list resources handler
    fn call(&self, params: ListResourcesRequestParams, args: Args) -> Self::Future;
}

/// Represents a generic completion handler, as registered by
/// [`App::map_completion`](crate::App::map_completion).
///
/// The completion counterpart of [`ListResourcesHandler`], with the same two
/// shapes.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid completion handler",
    label = "not a completion handler",
    note = "a completion handler takes `CompleteRequestParams` followed by extractable arguments \
            and returns either a value that converts into `CompleteResult` or a future of one"
)]
pub trait CompletionHandler<Args, M = marker::Async>: Clone + Send + Sync + 'static {
    /// Output type
    type Output;
    /// Output future
    type Future: Future<Output = Self::Output> + Send;

    /// Calls the completion handler
    fn call(&self, params: CompleteRequestParams, args: Args) -> Self::Future;
}

pub(crate) struct RequestFunc<F, R, Args, M>
where
    F: GenericHandler<Args, M, Output = R>,
    R: IntoResponse,
    Args: FromHandlerParams,
{
    func: F,
    // See `ToolFunc`: a function pointer keeps the phantom marker from
    // dragging auto traits onto the `Arc`-ed handler.
    _marker: std::marker::PhantomData<fn() -> (Args, M)>,
}

impl<F, R, Args, M> RequestFunc<F, R, Args, M>
where
    F: GenericHandler<Args, M, Output = R>,
    R: IntoResponse,
    Args: FromHandlerParams,
{
    pub(crate) fn new(func: F) -> Arc<Self> {
        let func = Self {
            func,
            _marker: std::marker::PhantomData,
        };
        Arc::new(func)
    }
}

impl<F, R, Args, M> Handler<Response> for RequestFunc<F, R, Args, M>
where
    F: GenericHandler<Args, M, Output = R>,
    R: IntoResponse,
    Args: FromHandlerParams + Send + Sync,
{
    #[inline]
    fn call(&self, params: HandlerParams) -> BoxFuture<'_, Result<Response, Error>> {
        Box::pin(async move {
            let id = RequestId::from_params(&params)?;
            let args = Args::from_params(&params)?;
            Ok(self.func.call(args).await.into_response(id))
        })
    }
}

impl FromHandlerParams for () {
    fn from_params(_: &HandlerParams) -> Result<Self, Error> {
        Ok(())
    }
}

impl FromHandlerParams for RequestId {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        match params {
            HandlerParams::Request(_, req) => Ok(req.id()),
            _ => Err(Error::new(
                ErrorCode::InternalError,
                "invalid handler parameters",
            )),
        }
    }
}

impl FromHandlerParams for Context {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        match params {
            HandlerParams::Request(context, _) => Ok(context.clone()),
            _ => Err(Error::new(
                ErrorCode::InternalError,
                "invalid handler parameters",
            )),
        }
    }
}

impl FromHandlerParams for RuntimeMcpOptions {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        match params {
            HandlerParams::Request(ctx, _) => Ok(ctx.options.clone()),
            _ => Err(Error::new(
                ErrorCode::InternalError,
                "invalid handler parameters",
            )),
        }
    }
}

/// The whole request, for a handler that asks for it, and so a copy of it.
/// Every other extractor reads the request in place.
impl FromHandlerParams for Request {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        match params {
            HandlerParams::Request(_, req) => Ok(Request::clone(req)),
            _ => Err(Error::new(
                ErrorCode::InternalError,
                "invalid handler parameters",
            )),
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
    impl<Func, Fut: Send, $($param,)*> HandlerFn<($($param,)*), marker::Async> for Func
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
    impl<Func, R: Send + 'static, $($param,)*> HandlerFn<($($param,)*), marker::Immediate> for Func
    where
        Func: Fn($($param),*) -> R + Send + Sync + Clone + 'static,
    {
        type Output = R;
        type Future = std::future::Ready<R>;

        #[inline]
        #[allow(non_snake_case)]
        fn call(&self, ($($param,)*): ($($param,)*)) -> Self::Future {
            std::future::ready((self)($($param,)*))
        }
    }
    // The offloaded shape. It needs no marker of its own: `BlockingFn` is a
    // distinct type and implements no other, so nothing can overlap here.
    impl<Func, R: Send + 'static, $($param,)*> HandlerFn<($($param,)*), marker::Immediate> for BlockingFn<Func>
    where
        Func: Fn($($param),*) -> R + Send + Sync + Clone + 'static,
        $($param: Send + 'static,)*
    {
        type Output = R;
        type Future = BlockingCall<R>;

        #[inline]
        #[allow(non_snake_case)]
        fn call(&self, ($($param,)*): ($($param,)*)) -> Self::Future {
            let func = self.0.clone();
            BlockingCall {
                handle: tokio::task::spawn_blocking(move || (func)($($param,)*)),
            }
        }
    }
    impl<Func, Fut: Send, $($param,)*> GenericHandler<($($param,)*), marker::Async> for Func
    where
        Func: Fn($($param),*) -> Fut + Send + Sync + Clone + 'static,
        Fut: Future + 'static,
    {}
    // The synchronous shape. `R: IntoResponse` is what keeps this impl and the
    // asynchronous one apart during selection -- see `HandlerFn` -- so it must
    // stay here rather than move to `App::map_handler`.
    impl<Func, R, $($param,)*> GenericHandler<($($param,)*), marker::Immediate> for Func
    where
        Func: Fn($($param),*) -> R + Send + Sync + Clone + 'static,
        R: IntoResponse + Send + 'static,
    {}
    impl<Func, R, $($param,)*> GenericHandler<($($param,)*), marker::Immediate> for BlockingFn<Func>
    where
        Func: Fn($($param),*) -> R + Send + Sync + Clone + 'static,
        R: IntoResponse + Send + 'static,
        $($param: Send + 'static,)*
    {}
    impl<Func, Fut: Send, $($param,)*> ListResourcesHandler<($($param,)*), marker::Async> for Func
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
    // The synchronous shape. `R: Into<ListResourcesResult>` is what keeps this
    // impl and the asynchronous one apart during selection -- see `HandlerFn`
    // -- so it must stay here rather than move to `App::map_resources`.
    impl<Func, R, $($param,)*> ListResourcesHandler<($($param,)*), marker::Immediate> for Func
    where
        Func: Fn(ListResourcesRequestParams, $($param),*) -> R + Send + Sync + Clone + 'static,
        R: Into<ListResourcesResult> + Send + 'static,
    {
        type Output = R;
        type Future = std::future::Ready<R>;

        #[inline]
        #[allow(non_snake_case)]
        fn call(&self, params: ListResourcesRequestParams, ($($param,)*): ($($param,)*)) -> Self::Future {
            std::future::ready((self)(params, $($param,)*))
        }
    }
    impl<Func, R, $($param,)*> ListResourcesHandler<($($param,)*), marker::Immediate> for BlockingFn<Func>
    where
        Func: Fn(ListResourcesRequestParams, $($param),*) -> R + Send + Sync + Clone + 'static,
        R: Into<ListResourcesResult> + Send + 'static,
        $($param: Send + 'static,)*
    {
        type Output = R;
        type Future = BlockingCall<R>;

        #[inline]
        #[allow(non_snake_case)]
        fn call(&self, params: ListResourcesRequestParams, ($($param,)*): ($($param,)*)) -> Self::Future {
            let func = self.0.clone();
            BlockingCall {
                handle: tokio::task::spawn_blocking(move || (func)(params, $($param,)*)),
            }
        }
    }
    impl<Func, Fut: Send, $($param,)*> CompletionHandler<($($param,)*), marker::Async> for Func
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
    // As above, with `R: Into<CompleteResult>` doing the separating.
    impl<Func, R, $($param,)*> CompletionHandler<($($param,)*), marker::Immediate> for Func
    where
        Func: Fn(CompleteRequestParams, $($param),*) -> R + Send + Sync + Clone + 'static,
        R: Into<CompleteResult> + Send + 'static,
    {
        type Output = R;
        type Future = std::future::Ready<R>;

        #[inline]
        #[allow(non_snake_case)]
        fn call(&self, params: CompleteRequestParams, ($($param,)*): ($($param,)*)) -> Self::Future {
            std::future::ready((self)(params, $($param,)*))
        }
    }
    impl<Func, R, $($param,)*> CompletionHandler<($($param,)*), marker::Immediate> for BlockingFn<Func>
    where
        Func: Fn(CompleteRequestParams, $($param),*) -> R + Send + Sync + Clone + 'static,
        R: Into<CompleteResult> + Send + 'static,
        $($param: Send + 'static,)*
    {
        type Output = R;
        type Future = BlockingCall<R>;

        #[inline]
        #[allow(non_snake_case)]
        fn call(&self, params: CompleteRequestParams, ($($param,)*): ($($param,)*)) -> Self::Future {
            let func = self.0.clone();
            BlockingCall {
                handle: tokio::task::spawn_blocking(move || (func)(params, $($param,)*)),
            }
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
    use super::*;
    use crate::App;
    use crate::transport::TransportProtoSender;
    use crate::types::ListToolsRequestParams;
    use serde_json::json;

    fn params(req: &Arc<Request>) -> HandlerParams {
        let runtime = App::new().build_runtime(TransportProtoSender::None);
        #[cfg(not(feature = "http-server"))]
        let context = runtime.context(None);
        #[cfg(feature = "http-server")]
        let context = runtime.context(None, Default::default(), None);
        HandlerParams::Request(context, Arc::clone(req))
    }

    fn call(params: serde_json::Value) -> Arc<Request> {
        Arc::new(Request::new(
            Some(RequestId::Number(7)),
            "tools/call",
            Some(params),
        ))
    }

    #[test]
    fn params_are_read_from_the_shared_request() {
        let req = call(json!({ "name": "greet", "arguments": { "who": "Ada" } }));
        let params = params(&req);

        let call = CallToolRequestParams::from_params(&params).expect("well-formed params");
        assert_eq!(call.name, "greet");
        assert_eq!(call.args.expect("arguments")["who"], "Ada");
        assert_eq!(
            RequestId::from_params(&params).expect("an id"),
            RequestId::Number(7)
        );
    }

    /// As `FromRequest` does: no params at all is an empty object, which a
    /// params type of optional fields reads as its defaults.
    #[test]
    fn absent_params_read_as_an_empty_object() {
        let req = Arc::new(Request::new(
            Some(RequestId::Number(1)),
            "tools/list",
            None::<()>,
        ));
        let params = params(&req);

        let list = ListToolsRequestParams::from_params(&params).expect("defaults");
        assert!(list.cursor.is_none());

        let err = CallToolRequestParams::from_params(&params).expect_err("`name` is required");
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[test]
    fn params_of_the_wrong_shape_are_invalid_params() {
        let req = call(json!({ "name": 42 }));

        let err = CallToolRequestParams::from_params(&params(&req)).expect_err("not a string");
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    /// The handler keeps nothing of the request once it is done: what the
    /// dispatcher holds is the request again, whole.
    #[tokio::test]
    async fn a_handler_leaves_the_request_to_the_dispatcher() {
        let handler = RequestFunc::new(|call: CallToolRequestParams| async move { call.name });
        let req = call(json!({ "name": "greet" }));

        let resp = handler.call(params(&req)).await.expect("a response");
        assert_eq!(resp.id(), &RequestId::Number(7));
        assert_eq!(Arc::strong_count(&req), 1);
        assert!(req.params.is_some());
    }
}

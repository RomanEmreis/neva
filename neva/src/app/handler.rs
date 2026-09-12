//! Handler utilities for resources, tools and prompts

use crate::Context;
use crate::app::options::RuntimeMcpOptions;
use crate::error::{Error, ErrorCode};
use crate::shared::BoxFuture;
use crate::types::{
    ArgNames, CallToolRequestParams, CompleteRequestParams, CompleteResult, GetPromptRequestParams,
    IntoResponse, ListResourcesRequestParams, ListResourcesResult, ReadResourceRequestParams,
    Request, RequestId, Response,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};

/// Represents a specific registered handler
pub(crate) type RequestHandler<T> = Arc<dyn Handler<T> + Send + Sync>;

/// Parameters handed to a registered handler.
///
/// The `Tool` and `Prompt` variants carry the [`ArgNames`] of the primitive
/// they were dispatched to, because extraction reads the request's
/// `arguments` map by name and only the registered [`crate::types::Tool`] /
/// [`crate::types::prompt::Prompt`] knows what its handler's arguments are
/// called.
#[derive(Debug)]
pub enum HandlerParams {
    Request(Context, Request),
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

/// Type-level markers telling the two shapes of handler apart.
///
/// A handler is recognized by its signature alone, and the two shapes -- one
/// returning a future, one returning its value directly -- cannot be separated
/// by a `where` clause: an impl for each would overlap and coherence rejects
/// that. Carrying the shape as a type parameter keeps the impls distinct, and
/// the marker is inferred at the registration site, so it does not appear in
/// handler code.
///
/// # Which shape to write
///
/// - The body **awaits** something -- an HTTP call, an async database driver,
///   another MCP peer through [`Context`] -- write an `async fn` or a closure
///   returning an `async` block. ([`marker::Async`])
/// - The body is **computation on data already in hand** -- arithmetic,
///   formatting, a lookup in a map, filtering a `Vec` -- write a plain `fn`.
///   It runs inline, with no task spawn and no yield point, and that is the
///   cheapest thing the server can do. ([`marker::Immediate`])
/// - The body **blocks** -- `std::fs`, a synchronous database or HTTP client,
///   `Command::output`, a long computation -- write a plain `fn` and register
///   it through [`blocking`](crate::blocking) or the `blocking` attribute.
///   Left inline, it would hold a runtime worker for its whole duration, and
///   that worker polls nothing else meanwhile.
///
/// Reaching for `blocking` on a body of the second kind is a pessimization:
/// handing `a + b` to another thread costs far more than the addition.
///
/// # Examples
/// ```no_run
/// use neva::App;
///
/// # #[tokio::main]
/// # async fn main() {
/// let mut app = App::new();
///
/// // `marker::Async`: the handler returns a future, which the server awaits.
/// app.map_tool("greet_later", |name: String| async move { format!("Hello, {name}") });
///
/// // `marker::Immediate`: the handler returns the value itself.
/// app.map_tool("greet", |name: String| format!("Hello, {name}"));
///
/// # app.run().await;
/// # }
/// ```
pub mod marker {
    /// Marks a handler that returns a [`Future`](std::future::Future) for the
    /// server to await.
    ///
    /// This is the default marker of every handler trait, so a bound written
    /// without one -- `F: ToolHandler<Args>` -- means exactly what it meant
    /// before synchronous handlers existed.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::App;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut app = App::new();
    ///
    /// app.map_tool("greet", |name: String| async move { format!("Hello, {name}") });
    ///
    /// # app.run().await;
    /// # }
    /// ```
    #[derive(Debug)]
    pub struct Async;

    /// Marks a handler that returns its value directly, with nothing to await.
    ///
    /// Such a handler runs to completion on the runtime thread that dispatched
    /// the request. That is right for computation and lookups; a handler that
    /// genuinely blocks -- file or socket I/O, a synchronous database driver,
    /// a long computation -- should either be asynchronous or be wrapped in
    /// [`blocking`](crate::blocking), which moves it off that thread.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::App;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut app = App::new();
    ///
    /// app.map_tool("greet", |name: String| format!("Hello, {name}"));
    ///
    /// # app.run().await;
    /// # }
    /// ```
    #[derive(Debug)]
    pub struct Immediate;
}

/// Runs a synchronous handler on Tokio's blocking pool instead of the runtime
/// thread that dispatched the request.
///
/// A [`marker::Immediate`] handler runs inline, which is right for computation
/// and lookups and wrong for anything that actually blocks -- file or socket
/// I/O, a synchronous database driver, a long computation. Blocking a runtime
/// worker stalls every other request it was going to poll. Wrapping the
/// handler here moves the body to [`tokio::task::spawn_blocking`] and awaits
/// it, so the worker stays free.
///
/// The trade is a hand-off to another thread, which costs far more than a
/// short body does: reach for this only when the body really blocks. See
/// [`marker`] for the three cases side by side.
///
/// The offloaded task is **not cancelled** if the request is: once started it
/// runs to completion, and its result is discarded.
///
/// The wrapper is accepted at every registration point -- tools, prompts,
/// resource reads, resource listings, completions and plain request handlers --
/// so one adapter covers them all. The `#[tool(blocking)]` attribute applies it
/// for you.
///
/// It takes a *synchronous* handler: an asynchronous one has nothing to
/// offload -- it already yields -- and is rejected at compile time.
///
/// # Panics
///
/// A panic inside the handler is propagated to the awaiting task, exactly as
/// it would be if the handler had run inline.
///
/// # Examples
/// ```no_run
/// use neva::{App, blocking};
///
/// # #[tokio::main]
/// # async fn main() {
/// let mut app = App::new();
///
/// app.map_tool("read_file", blocking(|path: String| {
///     std::fs::read_to_string(path).unwrap_or_default()
/// }))
/// .with_arg_names(["path"]);
///
/// # app.run().await;
/// # }
/// ```
#[inline]
pub fn blocking<F>(handler: F) -> BlockingFn<F> {
    BlockingFn(handler)
}

/// A handler moved onto Tokio's blocking pool. Created by [`blocking`].
#[derive(Debug, Clone)]
pub struct BlockingFn<F>(F);

/// The future of a handler running on Tokio's blocking pool.
///
/// Resolves to whatever the handler returned. It is the future type every
/// [`BlockingFn`] handler produces, which is why it is nameable at all; there
/// is nothing to construct here directly.
#[derive(Debug)]
pub struct BlockingCall<R> {
    handle: tokio::task::JoinHandle<R>,
}

impl<R> Future for BlockingCall<R> {
    type Output = R;

    fn poll(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        // `JoinHandle` is `Unpin`, so this projection needs no pinning dance.
        match Pin::new(&mut self.get_mut().handle).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(value)) => Poll::Ready(value),
            // The handler panicked. Resuming here puts the panic on the task
            // that awaited it, which is where it would have landed had the
            // handler run inline -- the offload is not supposed to change what
            // a panicking handler does.
            Poll::Ready(Err(err)) if err.is_panic() => std::panic::resume_unwind(err.into_panic()),
            // Not reachable through `spawn_blocking` on a live runtime: the
            // task is never cancelled, and a handle is never detached here.
            Poll::Ready(Err(err)) => panic!("neva: blocking handler did not run: {err}"),
        }
    }
}

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

impl FromHandlerParams for Request {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        match params {
            HandlerParams::Request(_, req) => Ok(req.clone()),
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
mod tests {}

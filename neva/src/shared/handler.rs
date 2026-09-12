//! Handler shapes shared by the server and the client: the markers that tell a
//! synchronous handler from an asynchronous one, and the adapter that moves a
//! blocking one off the runtime.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};

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
///   another MCP peer through the server's `Context` -- write an `async fn` or
///   a closure returning an `async` block. ([`marker::Async`])
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
/// # #[cfg(feature = "server")]
/// # #[tokio::main]
/// # async fn main() {
/// use neva::App;
///
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
/// # #[cfg(not(feature = "server"))]
/// # fn main() {}
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
    /// # #[cfg(feature = "server")]
    /// # #[tokio::main]
    /// # async fn main() {
    /// use neva::App;
    ///
    /// let mut app = App::new();
    ///
    /// app.map_tool("greet", |name: String| async move { format!("Hello, {name}") });
    ///
    /// # app.run().await;
    /// # }
    /// # #[cfg(not(feature = "server"))]
    /// # fn main() {}
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
    /// # #[cfg(feature = "server")]
    /// # #[tokio::main]
    /// # async fn main() {
    /// use neva::App;
    ///
    /// let mut app = App::new();
    ///
    /// app.map_tool("greet", |name: String| format!("Hello, {name}"));
    ///
    /// # app.run().await;
    /// # }
    /// # #[cfg(not(feature = "server"))]
    /// # fn main() {}
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
/// # #[cfg(feature = "server")]
/// # #[tokio::main]
/// # async fn main() {
/// use neva::{App, blocking};
///
/// let mut app = App::new();
///
/// app.map_tool("read_file", blocking(|path: String| {
///     std::fs::read_to_string(path).unwrap_or_default()
/// }))
/// .with_arg_names(["path"]);
///
/// # app.run().await;
/// # }
/// # #[cfg(not(feature = "server"))]
/// # fn main() {}
/// ```
#[inline]
pub fn blocking<F>(handler: F) -> BlockingFn<F> {
    BlockingFn(handler)
}

/// A handler moved onto Tokio's blocking pool. Created by [`blocking`].
#[derive(Debug, Clone)]
pub struct BlockingFn<F>(pub(crate) F);

/// The future of a handler running on Tokio's blocking pool.
///
/// Resolves to whatever the handler returned. It is the future type every
/// [`BlockingFn`] handler produces, which is why it is nameable at all; there
/// is nothing to construct here directly.
#[derive(Debug)]
pub struct BlockingCall<R> {
    pub(crate) handle: tokio::task::JoinHandle<R>,
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

//! Shared utilities for server and client

#[cfg(any(feature = "server", feature = "client"))]
use tokio_util::sync::CancellationToken;

#[cfg(feature = "tracing")]
pub(crate) use message_registry::MessageRegistry;
#[cfg(any(feature = "server", feature = "client"))]
pub(crate) use requests_queue::PendingResponse;
#[cfg(any(feature = "server", feature = "client"))]
pub(crate) use requests_queue::RequestQueue;
#[cfg(feature = "http-server")]
pub(crate) use sse_session_registry::SseSessionRegistry;
#[cfg(all(feature = "tasks", feature = "server"))]
pub(crate) use task_tracker::TaskHandle;
// The tracker backs the server's task substrate; a client only holds one to
// host tasks for legacy server->client requests.
#[cfg(all(feature = "tasks", any(feature = "server", feature = "legacy-spec")))]
pub(crate) use task_tracker::TaskTracker;

pub(crate) use arc_slice::ArcSlice;
pub(crate) use arc_str::ArcStr;
pub(crate) use memchr::MemChr;

pub use either::Either;
pub use into_args::IntoArgs;
pub use one_or_many::OneOrMany;
#[cfg(feature = "tasks")]
pub use task_api::{TaskApi, wait_to_completion};

mod arc_slice;
mod arc_str;
mod either;
mod into_args;
mod memchr;
#[cfg(feature = "tracing")]
mod message_registry;
#[cfg(feature = "http-client")]
pub mod mt;
mod one_or_many;
#[cfg(any(feature = "server", feature = "client"))]
mod requests_queue;
#[cfg(feature = "http-server")]
mod sse_session_registry;
#[cfg(feature = "tasks")]
mod task_api;
#[cfg(all(feature = "tasks", any(feature = "server", feature = "legacy-spec")))]
mod task_tracker;

/// The future returned by neva's object-safe async traits -- a boxed,
/// `Send` future borrowing for `'a`.
///
/// Traits like `AuthorizationHandler` (client OAuth) and `RequestStateStore`
/// (MRTR idempotency) are stored behind
/// `Arc<dyn ...>`, which rules out `async fn` in the trait (not dyn-compatible),
/// so their methods return this instead. Owning the alias here means
/// implementing such a trait needs no `futures` dependency of your own -- and
/// no version of it kept in lockstep with neva's.
///
/// It is a plain alias for `Pin<Box<dyn Future<Output = T> + Send + 'a>>`
/// (identical to `futures_util::future::BoxFuture`), so `Box::pin(async { ... })`
/// is all an implementation has to write.
///
/// # Example
/// ```
/// use neva::shared::BoxFuture;
///
/// trait Greeter {
///     fn greet(&self) -> BoxFuture<'_, String>;
/// }
///
/// struct English;
///
/// impl Greeter for English {
///     fn greet(&self) -> BoxFuture<'_, String> {
///         Box::pin(async { "hello".into() })
///     }
/// }
/// ```
pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

#[inline]
#[cfg(any(feature = "server", feature = "client"))]
pub(crate) fn wait_for_shutdown_signal(token: CancellationToken) {
    tokio::spawn(async move {
        match wait_for_shutdown_signal_impl().await {
            // A shutdown signal actually arrived -- cancel the transport.
            Ok(_) => token.cancel(),
            // Failing to *register* the handler (e.g. a sandboxed
            // environment restricting signal APIs) must not tear the
            // transport down -- the watcher simply exits and process
            // lifecycle stays with whatever launched it.
            #[cfg(feature = "tracing")]
            Err(err) => tracing::error!(
                logger = "neva",
                "Unable to listen for shutdown signal: {}",
                err
            ),
            #[cfg(not(feature = "tracing"))]
            Err(_) => (),
        }
    });
}

#[inline]
#[cfg(any(feature = "server", feature = "client"))]
async fn wait_for_shutdown_signal_impl() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal as unix_signal};

        let mut terminate = unix_signal(SignalKind::terminate())?;

        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }

    #[cfg(not(unix))]
    {
        #[cfg(windows)]
        {
            use tokio::signal::windows;

            let mut ctrl_break = windows::ctrl_break()?;
            let mut ctrl_close = windows::ctrl_close()?;
            let mut ctrl_shutdown = windows::ctrl_shutdown()?;

            tokio::select! {
                result = tokio::signal::ctrl_c() => result,
                _ = ctrl_break.recv() => Ok(()),
                _ = ctrl_close.recv() => Ok(()),
                _ = ctrl_shutdown.recv() => Ok(()),
            }
        }

        #[cfg(not(windows))]
        {
            tokio::signal::ctrl_c().await
        }
    }
}

/// Which protocol generation the connected peer speaks -- the runtime
/// switch behind the dual-mode client (issue #84).
///
/// An 2026-07-28 client starts in 2026-07-28 mode (`server/discover`, stateless,
/// MRTR) and flips to legacy exactly once, in `Client::connect`'s
/// fallback, before any other traffic -- so nothing races the switch.
/// The legacy build has no switch: it is legacy by construction.
///
/// Cheap to clone; all clones observe the same flip.
#[cfg(all(feature = "client", not(feature = "legacy-spec")))]
#[derive(Clone, Debug, Default)]
pub(crate) struct PeerMode(std::sync::Arc<std::sync::atomic::AtomicBool>);

#[cfg(all(feature = "client", not(feature = "legacy-spec")))]
impl PeerMode {
    /// Marks the peer as a legacy (legacy) server.
    pub(crate) fn set_legacy(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }

    /// Whether the peer speaks the legacy (legacy) protocol.
    pub(crate) fn is_legacy(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire)
    }
}

/// MRTR-eligible client requests (the only ones that may receive an
/// `InputRequiredResult`).
#[cfg(not(feature = "legacy-spec"))]
pub(crate) fn is_mrtr_method(method: &str) -> bool {
    matches!(
        method,
        crate::types::tool::commands::CALL
            | crate::types::prompt::commands::GET
            | crate::types::resource::commands::READ
    )
}

#[cfg(test)]
mod box_future_tests {
    use super::BoxFuture;

    /// The alias must stay *the same type* as `futures_util`'s, not merely
    /// a similar one: downstream code that already spells out
    /// `futures_util::future::BoxFuture` in its trait impls keeps compiling,
    /// so adopting neva's own alias is not a breaking change. Passing a
    /// value of one type where the other is expected only compiles if they
    /// are identical.
    #[test]
    fn it_is_the_same_type_as_the_futures_util_alias() {
        fn ours<'a, T: 'a>(fut: futures_util::future::BoxFuture<'a, T>) -> BoxFuture<'a, T> {
            fut
        }
        fn theirs<'a, T: 'a>(fut: BoxFuture<'a, T>) -> futures_util::future::BoxFuture<'a, T> {
            fut
        }

        let fut: BoxFuture<'_, u8> = Box::pin(async { 42 });
        let fut = theirs(fut);
        let mut fut = ours(fut);

        let waker = std::task::Waker::noop();
        let mut cx = std::task::Context::from_waker(waker);
        assert_eq!(
            fut.as_mut().poll(&mut cx),
            std::task::Poll::Ready(42),
            "the boxed future must still drive to completion"
        );
    }
}

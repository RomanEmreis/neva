//! Shared utilities for server and client

#[cfg(any(feature = "server", feature = "client"))]
use tokio_util::sync::CancellationToken;

#[cfg(feature = "tracing")]
pub(crate) use message_registry::MessageRegistry;
#[cfg(any(feature = "server", feature = "client"))]
pub(crate) use requests_queue::RequestQueue;
#[cfg(feature = "http-server")]
pub(crate) use sse_session_registry::SseSessionRegistry;
#[cfg(all(feature = "tasks", feature = "server"))]
pub(crate) use task_tracker::TaskHandle;
#[cfg(feature = "tasks")]
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
#[cfg(feature = "tasks")]
mod task_tracker;

#[inline]
#[cfg(any(feature = "server", feature = "client"))]
pub(crate) fn wait_for_shutdown_signal(token: CancellationToken) {
    tokio::spawn(async move {
        match tokio::signal::ctrl_c().await {
            Ok(_) => (),
            #[cfg(feature = "tracing")]
            Err(err) => tracing::error!(
                logger = "neva",
                "Unable to listen for shutdown signal: {}",
                err
            ),
            #[cfg(not(feature = "tracing"))]
            Err(_) => (),
        }
        token.cancel();
    });
}

//! Shared utilities for server and client

#[cfg(any(feature = "server", feature = "client"))]
use tokio_util::sync::CancellationToken;

#[cfg(any(feature = "server", feature = "client"))]
pub(crate) use requests_queue::RequestQueue;
#[cfg(any(feature = "http-server", feature = "tracing"))]
pub(crate) use message_registry::MessageRegistry;
pub(crate) use arc_str::ArcStr;
pub(crate) use arc_slice::ArcSlice;
pub(crate) use memchr::MemChr;

pub use into_args::IntoArgs;

#[cfg(feature = "http-client")]
pub mod mt;
#[cfg(any(feature = "server", feature = "client"))]
mod requests_queue;
#[cfg(any(feature = "http-server", feature = "tracing"))]
mod message_registry;
mod arc_str;
mod arc_slice;
mod into_args;
mod memchr;

#[inline]
#[cfg(any(feature = "server", feature = "client"))]
pub(crate) fn wait_for_shutdown_signal(token: CancellationToken) {
    tokio::spawn(async move {
        match tokio::signal::ctrl_c().await {
            Ok(_) => (),
            #[cfg(feature = "tracing")]
            Err(err) => tracing::error!(
                logger = "neva",
                "Unable to listen for shutdown signal: {}", err),
            #[cfg(not(feature = "tracing"))]
            Err(_) => ()
        }
        token.cancel();
    });
}

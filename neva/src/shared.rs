//! Shared utilities for server and client

use tokio_util::sync::CancellationToken;
pub(crate) use requests_queue::RequestQueue;

pub(crate) mod requests_queue;
pub(crate) mod message_registry;

#[inline]
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
//! Shared utilities for server and client

#[cfg(any(feature = "server", feature = "client"))]
use tokio_util::sync::CancellationToken;

#[cfg(all(feature = "tracing", not(feature = "proto-2026-07-28-rc")))]
pub(crate) use message_registry::MessageRegistry;
#[cfg(any(feature = "server", feature = "client"))]
pub(crate) use requests_queue::PendingResponse;
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
#[cfg(all(feature = "tracing", not(feature = "proto-2026-07-28-rc")))]
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
        match wait_for_shutdown_signal_impl().await {
            // A shutdown signal actually arrived — cancel the transport.
            Ok(_) => token.cancel(),
            // Failing to *register* the handler (e.g. a sandboxed
            // environment restricting signal APIs) must not tear the
            // transport down — the watcher simply exits and process
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

/// Which protocol generation the connected peer speaks — the runtime
/// switch behind the dual-mode client (issue #84).
///
/// An RC-flagged client starts in RC mode (`server/discover`, stateless,
/// MRTR) and flips to legacy exactly once, in `Client::connect`'s
/// fallback, before any other traffic — so nothing races the switch.
/// The legacy build has no switch: it is legacy by construction.
///
/// Cheap to clone; all clones observe the same flip.
#[cfg(all(feature = "client", feature = "proto-2026-07-28-rc"))]
#[derive(Clone, Debug, Default)]
pub(crate) struct PeerMode(std::sync::Arc<std::sync::atomic::AtomicBool>);

#[cfg(all(feature = "client", feature = "proto-2026-07-28-rc"))]
impl PeerMode {
    /// Marks the peer as a legacy (pre-RC) server.
    pub(crate) fn set_legacy(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }

    /// Whether the peer speaks the legacy (pre-RC) protocol.
    pub(crate) fn is_legacy(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire)
    }
}

/// MRTR-eligible client requests (the only ones that may receive an
/// `InputRequiredResult`).
#[cfg(feature = "proto-2026-07-28-rc")]
pub(crate) fn is_mrtr_method(method: &str) -> bool {
    matches!(
        method,
        crate::types::tool::commands::CALL
            | crate::types::prompt::commands::GET
            | crate::types::resource::commands::READ
    )
}

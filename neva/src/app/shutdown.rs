//! Programmatic shutdown for [`App`].
//!
//! A server otherwise stops only on an OS signal (SIGINT / SIGTERM and the
//! Windows equivalents). That is the right default for a process whose whole
//! job is to be an MCP server, and no use at all for the two cases that are
//! not that: a test that has to observe an orderly shutdown, and neva embedded
//! in a larger service that owns its own lifecycle.
//!
//! [`ShutdownHandle`] is the second entry point. It composes with the signal
//! handler rather than replacing it -- whichever fires first wins -- so a
//! server that takes a handle still stops on Ctrl+C.

use super::App;
use crate::shared;
#[cfg(not(feature = "legacy-spec"))]
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// How long shutdown waits for live `subscriptions/listen` streams to answer
/// before the transport goes down regardless.
///
/// Only ever paid by a server that has a subscription open. It is a ceiling,
/// not a delay: the wait ends as soon as the last result is queued, which is
/// immediate in the ordinary case. Two seconds sits well inside the ten Volga
/// gives an in-flight connection during its own graceful shutdown, so the
/// response body a subscription is written onto is still open when the result
/// arrives.
#[cfg(not(feature = "legacy-spec"))]
pub(super) const DEFAULT_SHUTDOWN_DRAIN: std::time::Duration = std::time::Duration::from_secs(2);

/// How often the drain re-checks whether the subscriptions have finished.
///
/// Short enough not to add a visible tail to shutdown, long enough that the
/// poll is not a spin.
#[cfg(not(feature = "legacy-spec"))]
const DRAIN_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(5);

/// Stops a running [`App`] without an OS signal.
///
/// Clones share one signal: any clone calling [`shutdown`](Self::shutdown)
/// stops the server the handle came from.
///
/// Shutdown is *requested* by this handle, not completed by it. Await
/// [`App::run`] to know the server is actually finished.
#[cfg_attr(
    not(feature = "legacy-spec"),
    doc = "
Under MCP 2026-07-28 the server first ends its live `subscriptions/listen`
streams and lets their results reach the wire -- see
[`App::with_shutdown_drain`](crate::App::with_shutdown_drain), which caps how
long that is allowed to take."
)]
///
/// # Example
/// ```no_run
/// use neva::App;
///
/// # #[tokio::main]
/// # async fn main() {
/// let (app, shutdown) = App::new().with_shutdown();
///
/// let server = tokio::spawn(app.run());
///
/// // ... later, from anywhere:
/// shutdown.shutdown();
/// server.await.expect("the server task panicked");
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct ShutdownHandle {
    token: CancellationToken,
}

impl ShutdownHandle {
    /// Creates a handle backed by a fresh signal.
    ///
    /// # Example
    /// ```
    /// use neva::app::shutdown::ShutdownHandle;
    ///
    /// let shutdown = ShutdownHandle::new();
    /// assert!(!shutdown.is_shutdown_requested());
    /// ```
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
        }
    }

    /// Wraps an existing [`CancellationToken`], so the server stops on a
    /// signal some other subsystem already owns.
    ///
    /// # Example
    /// ```
    /// use neva::app::shutdown::ShutdownHandle;
    /// use tokio_util::sync::CancellationToken;
    ///
    /// let token = CancellationToken::new();
    /// let shutdown = ShutdownHandle::from_token(token.clone());
    ///
    /// token.cancel();
    /// assert!(shutdown.is_shutdown_requested());
    /// ```
    pub fn from_token(token: CancellationToken) -> Self {
        Self { token }
    }

    /// Requests shutdown of the server this handle was taken from.
    ///
    /// Idempotent: calling it again is a no-op. It returns as soon as the
    /// request is recorded -- the server drains after it.
    ///
    /// # Example
    /// ```
    /// use neva::app::shutdown::ShutdownHandle;
    ///
    /// let shutdown = ShutdownHandle::new();
    /// shutdown.shutdown();
    /// assert!(shutdown.is_shutdown_requested());
    /// ```
    pub fn shutdown(&self) {
        self.token.cancel();
    }

    /// Whether shutdown has been requested.
    ///
    /// Reports the request, not its completion: the server may still be
    /// draining.
    ///
    /// # Example
    /// ```
    /// use neva::app::shutdown::ShutdownHandle;
    ///
    /// let shutdown = ShutdownHandle::new();
    /// assert!(!shutdown.is_shutdown_requested());
    /// ```
    pub fn is_shutdown_requested(&self) -> bool {
        self.token.is_cancelled()
    }

    /// The underlying [`CancellationToken`], for wiring this signal into
    /// something else that already speaks `tokio-util`.
    ///
    /// # Example
    /// ```
    /// use neva::app::shutdown::ShutdownHandle;
    ///
    /// let shutdown = ShutdownHandle::new();
    /// let token = shutdown.token();
    ///
    /// shutdown.shutdown();
    /// assert!(token.is_cancelled());
    /// ```
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }
}

impl From<CancellationToken> for ShutdownHandle {
    #[inline]
    fn from(token: CancellationToken) -> Self {
        Self::from_token(token)
    }
}

impl App {
    /// Turns one shutdown request into the ordered teardown the spec asks for:
    /// end the subscriptions, let their results out, then stop the transport.
    ///
    /// The spec says a server ending a subscription on its own initiative
    /// **SHOULD** answer the `subscriptions/listen` request with its empty
    /// result before closing the stream. That result is produced by a handler
    /// and travels the same channel as everything else, so it only lands if
    /// the writers are still reading when it is written -- which is exactly
    /// what one shared token made impossible.
    ///
    /// The wait is skipped when nothing was subscribed, so a server that never
    /// opens a subscription shuts down as immediately as it always did.
    #[cfg(not(feature = "legacy-spec"))]
    pub(super) fn relay_shutdown(
        shutdown: CancellationToken,
        subscriptions_token: CancellationToken,
        transport_token: CancellationToken,
        subscriptions: crate::app::subscriptions::SubscriptionRegistry,
        in_flight: Arc<std::sync::atomic::AtomicUsize>,
        drain: std::time::Duration,
    ) {
        tokio::spawn(async move {
            tokio::select! {
                // The transport going down on its own (a bind failure, a dead
                // engine) ends this task too -- otherwise it would sit on a
                // signal that is never coming.
                _ = transport_token.cancelled() => return,
                _ = shutdown.cancelled() => {}
            }

            // Phase 1: end the subscriptions. Each listen handler wakes,
            // deregisters, drains what its stream still owes and answers.
            let owed = !subscriptions.is_empty();
            subscriptions_token.cancel();

            // Phase 2: wait for those answers to reach the outbound channel.
            // `is_empty` says every handler deregistered; the in-flight count
            // says every one of them also got its response as far as the
            // sender, since the terminal middleware awaits that send before it
            // returns. Both, because either alone is reached too early.
            if owed {
                let _ = tokio::time::timeout(drain, async {
                    while !subscriptions.is_empty()
                        || in_flight.load(std::sync::atomic::Ordering::Acquire) > 0
                    {
                        tokio::time::sleep(DRAIN_POLL_INTERVAL).await;
                    }
                })
                .await;
            }

            // Phase 3: stop the transport. Its writers drain what is queued
            // before they exit, which is what carries the results written in
            // phase 2 onto the wire.
            transport_token.cancel();
        });
    }

    /// The legacy profile has no `subscriptions/listen` and so nothing to
    /// drain: shutdown reaches the transport directly.
    #[cfg(feature = "legacy-spec")]
    pub(super) fn relay_shutdown(shutdown: CancellationToken, transport_token: CancellationToken) {
        tokio::spawn(async move {
            tokio::select! {
                _ = transport_token.cancelled() => return,
                _ = shutdown.cancelled() => {}
            }
            transport_token.cancel();
        });
    }

    #[inline]
    pub(super) fn wait_for_shutdown_signal(&mut self, token: CancellationToken) {
        shared::wait_for_shutdown_signal(token);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_handle_has_not_been_fired() {
        assert!(!ShutdownHandle::new().is_shutdown_requested());
    }

    #[test]
    fn clones_share_one_signal() {
        let shutdown = ShutdownHandle::new();
        let clone = shutdown.clone();

        clone.shutdown();

        assert!(shutdown.is_shutdown_requested());
    }

    #[test]
    fn shutting_down_twice_is_a_no_op() {
        let shutdown = ShutdownHandle::new();

        shutdown.shutdown();
        shutdown.shutdown();

        assert!(shutdown.is_shutdown_requested());
    }

    #[test]
    fn a_wrapped_token_is_the_same_signal_in_both_directions() {
        let token = CancellationToken::new();
        let shutdown = ShutdownHandle::from_token(token.clone());

        token.cancel();
        assert!(shutdown.is_shutdown_requested());

        let other = ShutdownHandle::new();
        let watched = other.token();
        other.shutdown();
        assert!(watched.is_cancelled());
    }
}

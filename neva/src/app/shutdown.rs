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
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

/// Where the shutdown budget starts running out.
///
/// Stamped by the relay the moment a shutdown request arrives and read by
/// [`App::run`] when it comes to wait for the transport writers, so the two
/// halves of the teardown share one deadline instead of each taking the budget
/// afresh. Unset means no request ever reached the relay -- the transport went
/// down on its own -- and the writers are given the whole budget.
pub(super) type DrainDeadline = Arc<OnceLock<Instant>>;

/// What is left of the shutdown budget for the transport writers.
///
/// An unstamped deadline means no shutdown request reached the relay -- the
/// transport went down on its own -- so nothing has been spent and the writers
/// get the whole budget. A deadline already past yields
/// [`Duration::ZERO`](std::time::Duration::ZERO), which is one poll and no
/// waiting, the same thing opting out of the drain asks for.
#[inline]
pub(super) fn remaining_drain(
    deadline: &DrainDeadline,
    budget: std::time::Duration,
) -> std::time::Duration {
    deadline.get().map_or(budget, |deadline| {
        deadline.saturating_duration_since(Instant::now())
    })
}

/// How long shutdown waits for live `subscriptions/listen` streams to answer,
/// and then for the transport writers to put those answers on the wire, before
/// the server goes down regardless.
///
/// One budget covering both halves of the teardown, since both are the same
/// question -- how long a client's last answer is worth waiting for. It is a
/// ceiling, not a delay: the first half is skipped outright when no
/// subscription is open, and the second ends the moment the writers run dry.
/// Two seconds sits well inside the ten Volga gives an in-flight connection
/// during its own graceful shutdown, so the response body a subscription is
/// written onto is still open when the result arrives.
///
/// Under the legacy profile there are no subscriptions to answer, so only the
/// writer half is ever paid.
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
        deadline: DrainDeadline,
    ) {
        tokio::spawn(async move {
            tokio::select! {
                // The transport going down on its own (a bind failure, a dead
                // engine) ends this task too -- otherwise it would sit on a
                // signal that is never coming.
                _ = transport_token.cancelled() => return,
                _ = shutdown.cancelled() => {}
            }

            // The budget starts here, and it is the same budget the writer
            // wait in `run` spends the remainder of. Two phases, one deadline:
            // `with_shutdown_drain` says how long a client's last answer is
            // worth waiting for, not how long each half of the teardown may
            // take.
            let _ = deadline.set(Instant::now() + drain);

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
            // phase 2 onto the wire -- and `App::run` waits for that drain to
            // finish rather than returning into it, so the runtime a caller
            // drops next cannot abort it half-written.
            transport_token.cancel();
        });
    }

    /// The legacy profile has no `subscriptions/listen` and so nothing to
    /// drain: shutdown reaches the transport directly.
    #[cfg(feature = "legacy-spec")]
    pub(super) fn relay_shutdown(
        shutdown: CancellationToken,
        transport_token: CancellationToken,
        drain: std::time::Duration,
        deadline: DrainDeadline,
    ) {
        tokio::spawn(async move {
            tokio::select! {
                _ = transport_token.cancelled() => return,
                _ = shutdown.cancelled() => {}
            }
            // Nothing to drain ahead of the transport here, so the whole
            // budget is the writers' -- stamped all the same, so `run` reads
            // one deadline in both profiles.
            let _ = deadline.set(Instant::now() + drain);
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
    use std::time::Duration;

    /// No shutdown request ever arrived -- the transport failed under the
    /// server -- so nothing has been spent and the writers get all of it.
    #[test]
    fn an_unstamped_deadline_leaves_the_whole_budget() {
        let deadline = DrainDeadline::default();

        assert_eq!(
            remaining_drain(&deadline, Duration::from_secs(2)),
            Duration::from_secs(2)
        );
    }

    /// The two halves of the teardown share one budget: what the wait for the
    /// subscriptions spent is not offered to the writers a second time.
    #[test]
    fn a_stamped_deadline_is_what_is_left_of_it() {
        let deadline = DrainDeadline::default();
        let _ = deadline.set(Instant::now() + Duration::from_secs(2));

        let remaining = remaining_drain(&deadline, Duration::from_secs(2));

        assert!(
            remaining <= Duration::from_secs(2),
            "the remainder cannot exceed the budget it came from"
        );
        assert!(
            remaining > Duration::from_millis(1500),
            "and a deadline just stamped has almost all of it left"
        );
    }

    /// A budget already spent is not a fresh one: the writers get one poll.
    #[test]
    fn a_deadline_already_past_leaves_nothing() {
        let deadline = DrainDeadline::default();
        let _ = deadline.set(Instant::now() - Duration::from_secs(1));

        assert_eq!(
            remaining_drain(&deadline, Duration::from_secs(2)),
            Duration::ZERO
        );
    }

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

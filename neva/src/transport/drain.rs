//! The completion half of a transport shutdown.
//!
//! Cancelling a transport's [`CancellationToken`] tells its writers to stop
//! taking new work. It says nothing about the work they already had: both the
//! stdio writer and the HTTP dispatch pump keep going after cancellation until
//! the queue behind them is empty, because a result a handler produced during
//! the shutdown drain -- the graceful close of a `subscriptions/listen` stream
//! -- is written in exactly that window.
//!
//! That second half of the teardown happens in detached tasks, so nothing
//! joined it to [`App::run`](crate::App::run) returning. Under
//! [`App::run_blocking`](crate::App::run_blocking) the runtime is dropped the
//! moment `run` returns, and dropping a Tokio runtime aborts tasks that have
//! not finished -- cutting off the very drain that exists to get those results
//! onto the wire.
//!
//! [`DrainSignal`] closes that gap: every writer holds a [`DrainGuard`] for as
//! long as it may still write, and [`DrainSignal::wait`] resolves once the last
//! one is gone.
//!
//! [`CancellationToken`]: tokio_util::sync::CancellationToken

use std::time::Duration;
use tokio::sync::mpsc;

/// Held by a transport writer for as long as it may still write.
///
/// Cloneable, so a transport with several writers hands one to each: the
/// signal is raised when the last of them is dropped, not the first.
#[derive(Clone, Debug)]
pub(crate) struct DrainGuard {
    /// Never sent on. The signal is the drop of the last clone, which closes
    /// the channel -- that, and not a message, is what completes
    /// [`DrainSignal::wait`].
    _tx: mpsc::Sender<()>,
}

/// The waiting half of the drain signal: completes once every [`DrainGuard`]
/// taken from it has been dropped.
///
/// Only the server waits -- [`App::run`](crate::App::run) is the one caller of
/// [`wait`](Self::wait) -- while a client transport produces the signal and
/// drops it, the writer being one piece of code serving both roles. Hence the
/// dead-code exemption on the waiting side in a client-only build.
#[derive(Debug)]
pub(crate) struct DrainSignal {
    #[cfg_attr(not(feature = "server"), allow(dead_code))]
    rx: mpsc::Receiver<()>,
}

impl DrainSignal {
    /// Creates a guard and the signal that waits on it.
    pub(crate) fn new() -> (DrainGuard, Self) {
        let (tx, rx) = mpsc::channel(1);
        (DrainGuard { _tx: tx }, Self { rx })
    }

    /// A signal that is already complete, for a transport with no writers of
    /// its own to wait for.
    pub(crate) fn ready() -> Self {
        let (guard, drained) = Self::new();
        drop(guard);
        drained
    }

    /// Waits until every [`DrainGuard`] has been dropped, or `budget` elapses.
    ///
    /// Returns whether the writers finished inside the budget. A budget of
    /// [`Duration::ZERO`] gives them one poll and no more, which is what
    /// opting out of the drain asks for.
    #[cfg_attr(not(feature = "server"), allow(dead_code))]
    pub(crate) async fn wait(mut self, budget: Duration) -> bool {
        // Nothing is ever sent on this channel, so the only way `recv`
        // resolves is the last guard dropping and closing it.
        tokio::time::timeout(budget, self.rx.recv()).await.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The signal is the guard's drop -- not a message, and not the guard
    /// merely existing.
    #[tokio::test]
    async fn it_completes_once_the_guard_is_dropped() {
        let (guard, drained) = DrainSignal::new();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            drop(guard);
        });

        assert!(drained.wait(Duration::from_secs(5)).await);
    }

    /// A writer that never finishes cannot hold shutdown open forever: the
    /// budget is a ceiling on the whole wait.
    #[tokio::test]
    async fn it_gives_up_on_a_writer_that_never_finishes() {
        let (guard, drained) = DrainSignal::new();

        assert!(!drained.wait(Duration::from_millis(20)).await);

        drop(guard);
    }

    /// Several writers, one signal: the last one out raises it.
    #[tokio::test]
    async fn it_waits_for_every_writer() {
        let (guard, drained) = DrainSignal::new();
        let second = guard.clone();

        drop(guard);
        assert!(
            !drained.wait(Duration::from_millis(20)).await,
            "one writer still holding a guard must keep the signal pending"
        );

        drop(second);
    }

    /// A transport with nothing to wait for does not make shutdown pay for the
    /// wait.
    #[tokio::test]
    async fn a_ready_signal_does_not_wait() {
        assert!(DrainSignal::ready().wait(Duration::from_secs(5)).await);
    }
}

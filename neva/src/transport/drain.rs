//! The completion half of a transport shutdown.
//!
//! Cancelling a transport's [`CancellationToken`] tells it to stop taking new
//! work. It says nothing about the work it already had: the stdio writer and
//! the HTTP dispatch pump both keep going after cancellation until the queue
//! behind them is empty, and the HTTP engine has its own graceful shutdown to
//! run before the bytes that pump routed are on the socket. A result a handler
//! produced during the shutdown drain -- the graceful close of a
//! `subscriptions/listen` stream -- is written in exactly that window.
//!
//! That second half of the teardown happens in detached tasks, so nothing
//! joined it to [`App::run`](crate::App::run) returning. Under
//! [`App::run_blocking`](crate::App::run_blocking) the runtime is dropped the
//! moment `run` returns, and dropping a Tokio runtime aborts tasks that have
//! not finished -- cutting off the very drain that exists to get those results
//! onto the wire.
//!
//! [`DrainSignal`] closes that gap: everything that may still write holds a
//! [`DrainGuard`] until it is done, and
//! [`DrainSignal::wait_or_abort`] resolves once the last one is gone -- or
//! stops them, if the shutdown budget runs out first. Returning from `run`
//! while a writer is still going would put the same output on stdout after the
//! server said it had stopped.
//!
//! [`CancellationToken`]: tokio_util::sync::CancellationToken

use std::time::Duration;
use tokio::{sync::mpsc, task::AbortHandle};

/// How long an abort is given to land before shutdown stops caring.
///
/// [`AbortHandle::abort`] is a request, not a stop: the task ends at its next
/// yield point, and only then is its future -- and the [`DrainGuard`] inside it
/// -- dropped. Waiting for that is what makes "stopped" true rather than
/// "asked to stop", so that a writer mid-poll cannot finish its write after
/// `run` has returned. Not a knob: what it covers is one poll of an
/// already-cancelled task, and a task that never yields at all must not be
/// able to hold open a shutdown that has already spent its budget.
const ABORT_GRACE: Duration = Duration::from_millis(100);

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
/// [`wait_or_abort`](Self::wait_or_abort) -- while a client transport produces
/// the signal and drops it, the writer being one piece of code serving both
/// roles. Hence the dead-code exemption on the waiting side in a client-only
/// build.
#[derive(Debug)]
pub(crate) struct DrainSignal {
    #[cfg_attr(not(feature = "server"), allow(dead_code))]
    rx: mpsc::Receiver<()>,
    /// The tasks behind the guards, so a budget that runs out ends them
    /// instead of merely giving up on them.
    #[cfg_attr(not(feature = "server"), allow(dead_code))]
    tasks: Vec<AbortHandle>,
}

impl DrainSignal {
    /// Creates a guard and the signal that waits on it.
    pub(crate) fn new() -> (DrainGuard, Self) {
        let (tx, rx) = mpsc::channel(1);
        (
            DrainGuard { _tx: tx },
            Self {
                rx,
                tasks: Vec::new(),
            },
        )
    }

    /// Registers the task behind one of the guards, to be aborted if the
    /// shutdown budget runs out before it finishes.
    ///
    /// A transport registers what it spawned; anything it did not spawn -- the
    /// stdio reader thread, which is not the runtime's to end -- stays out of
    /// this, as it always has.
    pub(crate) fn abort_on_timeout(&mut self, task: AbortHandle) {
        self.tasks.push(task);
    }

    /// A signal that is already complete, for a transport with no writers of
    /// its own to wait for.
    pub(crate) fn ready() -> Self {
        let (guard, drained) = Self::new();
        drop(guard);
        drained
    }

    /// Waits until every [`DrainGuard`] has been dropped, or `budget` elapses
    /// -- in which case the registered tasks are aborted.
    ///
    /// Returns whether they finished inside the budget. Giving up on the wait
    /// is not the same as giving up on the tasks: a writer left running past
    /// the budget goes on writing on a runtime that outlives the server, so
    /// output the shutdown decided not to wait for would land after `run`
    /// returned, on a stdout its host may have taken back. A budget of
    /// [`Duration::ZERO`] therefore is what it says it is -- one poll, then an
    /// abrupt close -- at the cost of a message the writer was midway through.
    #[cfg_attr(not(feature = "server"), allow(dead_code))]
    pub(crate) async fn wait_or_abort(mut self, budget: Duration) -> bool {
        // Nothing is ever sent on this channel, so the only way `recv`
        // resolves is the last guard dropping and closing it.
        if tokio::time::timeout(budget, self.rx.recv()).await.is_ok() {
            return true;
        }

        for task in &self.tasks {
            task.abort();
        }

        // The guards go down with the futures the abort drops, so the signal
        // that said "still writing" is also the one that says "gone".
        let _ = tokio::time::timeout(ABORT_GRACE, self.rx.recv()).await;
        false
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

        assert!(drained.wait_or_abort(Duration::from_secs(5)).await);
    }

    /// A writer that never finishes cannot hold shutdown open forever: the
    /// budget is a ceiling on the whole wait.
    #[tokio::test]
    async fn it_gives_up_on_a_writer_that_never_finishes() {
        let (guard, drained) = DrainSignal::new();

        assert!(!drained.wait_or_abort(Duration::from_millis(20)).await);

        drop(guard);
    }

    /// And giving up on the wait ends the writer rather than leaving it on the
    /// runtime, where it would write on after the server said it had stopped
    /// -- ended by the time the wait returns, too, since an abort is a request
    /// and a writer still being polled when it is made would otherwise finish
    /// the write it was in the middle of afterwards.
    #[tokio::test]
    async fn it_stops_the_writer_the_budget_ran_out_on() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        /// Set when the task's future is dropped -- the abort actually
        /// landing, rather than merely having been asked for.
        struct Stopped(Arc<AtomicBool>);

        impl Drop for Stopped {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let (guard, mut drained) = DrainSignal::new();
        let stopped = Arc::new(AtomicBool::new(false));

        let writer = tokio::spawn({
            let stopped = stopped.clone();
            async move {
                // Declared before the marker, so it is dropped after it: by
                // the time the guard's drop completes the wait below, the
                // marker has already been set.
                let _guard = guard;
                let _stopped = Stopped(stopped);
                std::future::pending::<()>().await;
            }
        });
        drained.abort_on_timeout(writer.abort_handle());

        assert!(!drained.wait_or_abort(Duration::from_millis(20)).await);
        assert!(
            stopped.load(Ordering::SeqCst),
            "the abort must have landed before the wait reported the writer stopped"
        );
        assert!(
            writer.await.is_err_and(|err| err.is_cancelled()),
            "a writer past the budget must be stopped, not abandoned"
        );
    }

    /// A writer that finished in time is left alone -- there is nothing to
    /// abort, and its task ended on its own terms.
    #[tokio::test]
    async fn it_leaves_a_writer_that_finished_in_time_alone() {
        let (guard, mut drained) = DrainSignal::new();

        let writer = tokio::spawn(async move {
            drop(guard);
            "written"
        });
        drained.abort_on_timeout(writer.abort_handle());

        assert!(drained.wait_or_abort(Duration::from_secs(5)).await);
        assert_eq!(
            writer.await.expect("the writer must not be aborted"),
            "written"
        );
    }

    /// Several writers, one signal: the last one out raises it.
    #[tokio::test]
    async fn it_waits_for_every_writer() {
        let (guard, drained) = DrainSignal::new();
        let second = guard.clone();

        drop(guard);
        assert!(
            !drained.wait_or_abort(Duration::from_millis(20)).await,
            "one writer still holding a guard must keep the signal pending"
        );

        drop(second);
    }

    /// A transport with nothing to wait for does not make shutdown pay for the
    /// wait.
    #[tokio::test]
    async fn a_ready_signal_does_not_wait() {
        assert!(
            DrainSignal::ready()
                .wait_or_abort(Duration::from_secs(5))
                .await
        );
    }
}

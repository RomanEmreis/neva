//! Programmatic shutdown for [`App`](crate::App).
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

use tokio_util::sync::CancellationToken;

/// Stops a running [`App`](crate::App) without an OS signal.
///
/// Clones share one signal: any clone calling [`shutdown`](Self::shutdown)
/// stops the server the handle came from.
///
/// Shutdown is *requested* by this handle, not completed by it. Await
/// [`App::run`](crate::App::run) to know the server is actually finished.
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

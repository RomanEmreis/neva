//! Utilities and helpers for cross-platform multithreading.

/// Cooperatively yields to the Tokio scheduler before spawning a new task,
/// ensuring fair task scheduling especially under high load or on platforms
/// like Windows where newly spawned tasks may not execute promptly.
///
/// This function should be used in cases where a tight loop repeatedly spawns
/// tasks (e.g., after reading from a channel), to prevent starving previously
/// scheduled tasks.
///
/// ## Example
/// ```rust
/// # use crate::shared::mt::spawn_fair;
/// # async fn dox() {
/// # let (tx, mut rx) = tokio::sync::mpsc::channel(10).split();
/// while let Some(msg) = rx.recv().await {
///     spawn_fair!(async move {
///         handle(msg).await;
///     });
/// }
/// # }
/// # async fn handle(msg: &str) {}
/// ```
///
/// ## Notes
/// - This is a drop-in replacement for `tokio::spawn`.
/// - On Windows, it inserts a short sleep after yielding to ensure fair scheduling.
#[macro_export]
macro_rules! spawn_fair {
    ($future:expr) => {{
        $crate::shared::mt::yield_fair().await;
        tokio::spawn($future);
    }};
}

/// Cooperatively yields execution back to the Tokio scheduler to allow other
/// tasks to progress.
///
/// This function is especially useful in tight loops that continuously spawn tasks
/// or perform high-throughput operations (e.g., reading from an `mpsc` channel
/// and spawning a task for each received message). Without explicitly yielding,
/// the current task may monopolize the executor thread, starving other tasks
/// that have been scheduled but haven't yet had a chance to run.
///
/// On **Linux** and other Unix-like platforms, `tokio::task::yield_now()`
/// is typically sufficient. It instructs the scheduler to suspend the current task
/// and allow other tasks to be polled. The scheduler on these platforms usually
/// reschedules tasks efficiently and fairly, so this form of yielding is often enough.
///
/// On **Windows**, however, the Tokio runtime may not reschedule tasks as aggressively,
/// particularly in `multi_thread` mode. In such cases, `yield_now()` alone might
/// not cause other tasks to be executed promptly. To address this, we follow it
/// with a zero-duration `sleep`, which forces a yield to the timer driver and
/// guarantees the task will be rescheduled on the next tick. We use a duration
/// of **1 millisecond** rather than zero, since `sleep(Duration::ZERO)` is known
/// to be unreliable on Windows due to limitations in the underlying timer API.
///
/// # Platform Behavior Summary
/// - **Linux / Unix:** `yield_now()` is sufficient and efficient.
/// - **Windows:** `yield_now()` followed by `sleep(1ms)` ensures fairness.
///
/// This method is intended to be called *before* spawning new tasks or
/// entering another blocking `await`, to prevent starvation of previously
/// scheduled tasks.
///
/// ## Example
/// ```no_run
/// # use crate::shared::mt::yield_fair;
/// # async fn dox() {
/// # let (tx, mut rx) = tokio::sync::mpsc::channel(10).split();
/// while let Some(msg) = rx.recv().await {
///     yield_fair().await;
///     tokio::spawn(handle(msg));
/// }
/// # }
/// # async fn handle(msg: &str) {}
/// ```
#[inline]
pub async fn yield_fair() {
    tokio::task::yield_now().await;
    #[cfg(target_os = "windows")]
    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
}
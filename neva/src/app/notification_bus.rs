//! Cross-instance fan-out for subscription notifications (MCP 2026-07-28).
//!
//! A `subscriptions/listen` stream is a socket held open by exactly one
//! process. Under the stateless 2026-07-28 HTTP transport nothing pins a client
//! to an instance, so the stream and the request that mutates the server
//! routinely land on different ones:
//!
//! ```text
//! client --- subscriptions/listen ------------> instance A   (stream held here)
//! client --- tools/call (mutates the tools) --> instance B   (ctx.add_tool)
//!                                               instance B has no subscribers
//!                                               instance A's subscriber hears nothing
//! ```
//!
//! The subscriber is told its filter was accepted, so the loss reads as "the
//! server never changes" rather than as a delivery failure.
//!
//! ## Why the registry is not the seam
//!
//! The registry behind those streams is a table of *live connections*, not
//! application state: each entry pairs a serializable id and filter with an
//! `mpsc::Sender` writing into one held-open response body and an in-process
//! cancellation token. A shared store could persist the first half and still
//! deliver nothing -- instance B cannot write into instance A's socket. So the
//! registry stays node-local by construction, and what is made pluggable is the
//! *distribution*: every instance keeps its own subscribers, a
//! [`NotificationBus`] carries notifications between instances, and each one
//! delivers to the subscribers it actually holds.
//!
//! This is the same split the crate already draws elsewhere:
//! [`RequestStateStore`](crate::app::mrtr_store::RequestStateStore) is a trait
//! with an in-memory default because it stores a plain `Response` under a
//! string key; the SSE session registry is a concrete in-process type because
//! it holds channel senders.
//!
//! ## The default
//!
//! No bus is configured by default, and a notification goes straight to this
//! instance's own subscribers -- no channel, no allocation, no task. A
//! single-instance server behaves exactly as it did before this trait existed,
//! and pays nothing for its presence.
//!
//! Shared implementations (Redis pub/sub, NATS, Postgres `LISTEN`/`NOTIFY`)
//! live outside this crate; neva ships the trait and the local default, the
//! same way it does for the MRTR state store.
//!
//! # Examples
//! ```no_run
//! # #[cfg(not(feature = "legacy-spec"))] {
//! use neva::App;
//! use neva::app::notification_bus::NotificationBus;
//! use neva::shared::{BoxFuture, BoxStream};
//! use tokio::sync::broadcast::{Sender, channel};
//!
//! /// Stands in for a real bus: one process-wide channel every instance
//! /// publishes to and reads back from, echo included.
//! struct BroadcastBus(Sender<(String, Option<serde_json::Value>)>);
//!
//! impl NotificationBus for BroadcastBus {
//!     fn publish<'a>(
//!         &'a self,
//!         method: &'a str,
//!         params: Option<&'a serde_json::Value>,
//!     ) -> BoxFuture<'a, ()> {
//!         let msg = (method.to_owned(), params.cloned());
//!         Box::pin(async move {
//!             // No subscriber yet is not an error: nobody is listening.
//!             let _ = self.0.send(msg);
//!         })
//!     }
//!
//!     fn subscribe(&self) -> BoxStream<'static, (String, Option<serde_json::Value>)> {
//!         let rx = self.0.subscribe();
//!         Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
//!             rx.recv().await.ok().map(|msg| (msg, rx))
//!         }))
//!     }
//! }
//!
//! let (tx, _) = channel(64);
//! let app = App::new().with_notification_bus(BroadcastBus(tx));
//! # }
//! ```

use crate::shared::{BoxFuture, BoxStream};

/// Carries subscribable notifications between the instances of one logical
/// server (MCP 2026-07-28).
///
/// Implement this to fan `tools`/`prompts`/`resources` list-changed and
/// `resources/updated` notifications out across a horizontally scaled
/// deployment, and install it with
/// [`App::with_notification_bus`](crate::App::with_notification_bus). Only
/// notification types a client can subscribe to reach the bus; progress, task
/// status and elicitation stay request-scoped and never travel on one.
///
/// See the [module docs](self) for why the subscriber table itself stays
/// node-local.
///
/// ## Contract
///
/// * **No echo suppression.** [`subscribe`](Self::subscribe) must yield the
///   notifications this instance published as well as everybody else's. Local
///   delivery happens through that stream and only through it, so a bus that
///   hides an instance's own messages from it silences that instance's own
///   subscribers. (Redis pub/sub, NATS and a `tokio::sync::broadcast` channel
///   all echo by default; suppressing it takes deliberate work.)
/// * **At-most-once.** A subscription whose buffer is full drops the
///   notification with a warning rather than blocking the request that produced
///   it, so a bus must not promise more than the sink it feeds. Redelivery
///   after an instance dies buys nothing either: subscriptions are not
///   resumable by spec, and a client whose stream drops re-sends
///   `subscriptions/listen`.
/// * **Ordering.** The "acknowledgment MUST be first" rule is per-subscription
///   and stays entirely local -- the acknowledgment is queued before the
///   registry entry goes live, so no notification can overtake it however it
///   arrives. Cross-instance ordering between *different* notifications is not
///   something the spec requires.
/// * **Cost.** [`publish`](Self::publish) is awaited inside the request that
///   produced the notification, so a slow bus slows that request down. Prefer
///   an implementation that hands off to a background connection over one that
///   waits for a round trip.
///
/// # Examples
/// See the [module docs](self) for a complete implementation.
pub trait NotificationBus: Send + Sync {
    /// Publishes a notification produced on this instance.
    ///
    /// Fan-out only: the notification is *not* delivered locally by this call.
    /// It comes back to every instance -- this one included -- through
    /// [`subscribe`](Self::subscribe), which is what keeps local and remote
    /// delivery on one code path.
    fn publish<'a>(
        &'a self,
        method: &'a str,
        params: Option<&'a serde_json::Value>,
    ) -> BoxFuture<'a, ()>;

    /// Yields the notifications published by any instance, this one's own
    /// included.
    ///
    /// Called once per server, at startup; the server drains the stream into
    /// its local subscribers until the stream ends or the server shuts down. A
    /// stream that ends stops delivery for the rest of the process's life, so
    /// an implementation that can reconnect should do so inside the stream
    /// rather than end it.
    fn subscribe(&self) -> BoxStream<'static, (String, Option<serde_json::Value>)>;
}

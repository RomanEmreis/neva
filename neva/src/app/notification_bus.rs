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
//! use neva::app::notification_bus::{BusNotification, NotificationBus};
//! use neva::shared::Stream;
//! use tokio::sync::broadcast::{Sender, channel, error::RecvError};
//!
//! /// Stands in for a real bus: one process-wide channel every instance
//! /// publishes to and reads back from, echo included.
//! struct BroadcastBus(Sender<BusNotification>);
//!
//! impl NotificationBus for BroadcastBus {
//!     async fn publish(&self, notification: BusNotification) {
//!         // Nobody draining yet is not an error worth failing a request over.
//!         let _ = self.0.send(notification);
//!     }
//!
//!     fn subscribe(&self) -> impl Stream<Item = BusNotification> + Send + 'static {
//!         let rx = self.0.subscribe();
//!         futures_util::stream::unfold(rx, |mut rx| async move {
//!             loop {
//!                 match rx.recv().await {
//!                     Ok(notification) => return Some((notification, rx)),
//!                     // At-most-once: skip what was missed rather than end
//!                     // delivery for good.
//!                     Err(RecvError::Lagged(_)) => continue,
//!                     Err(RecvError::Closed) => return None,
//!                 }
//!             }
//!         })
//!     }
//! }
//!
//! let (tx, _) = channel(64);
//! let app = App::new().with_notification_bus(BroadcastBus(tx));
//! # }
//! ```

use crate::shared::{BoxFuture, BoxStream, Stream};
use serde::{Deserialize, Serialize};

/// One subscribable notification in flight between instances
/// (MCP 2026-07-28).
///
/// Carries the JSON-RPC method and params of a `tools`/`prompts`/`resources`
/// list-changed or `resources/updated` notification -- and nothing about the
/// instance that produced it or the subscription it will end up on. Both are
/// decided on arrival: the receiving instance matches it against the filters of
/// the streams *it* holds and stamps each copy with that stream's own
/// subscription id.
///
/// It serializes as the notification body it describes (`{"method": ...,
/// "params": ...}`), so a bus that ships JSON can hand it straight to
/// `serde_json` in both directions rather than inventing an envelope.
///
/// # Examples
/// ```
/// # #[cfg(not(feature = "legacy-spec"))] {
/// use neva::app::notification_bus::BusNotification;
///
/// let notification = BusNotification::new(
///     "notifications/resources/updated",
///     Some(serde_json::json!({ "uri": "res://config" })),
/// );
///
/// let wire = serde_json::to_string(&notification).unwrap();
/// let back: BusNotification = serde_json::from_str(&wire).unwrap();
///
/// assert_eq!(back.method(), "notifications/resources/updated");
/// assert_eq!(back.params().unwrap()["uri"], "res://config");
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BusNotification {
    /// The JSON-RPC method, e.g. `notifications/tools/list_changed`.
    method: String,

    /// The notification's params, if it carries any. `resources/updated` is
    /// the one that always does -- it names the `uri` that changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

impl BusNotification {
    /// Creates a notification from its method and params.
    ///
    /// # Examples
    /// ```
    /// # #[cfg(not(feature = "legacy-spec"))] {
    /// use neva::app::notification_bus::BusNotification;
    ///
    /// let notification = BusNotification::new("notifications/tools/list_changed", None);
    /// assert_eq!(notification.method(), "notifications/tools/list_changed");
    /// assert!(notification.params().is_none());
    /// # }
    /// ```
    pub fn new(method: impl Into<String>, params: Option<serde_json::Value>) -> Self {
        Self {
            method: method.into(),
            params,
        }
    }

    /// Returns the JSON-RPC method.
    ///
    /// # Examples
    /// ```
    /// # #[cfg(not(feature = "legacy-spec"))] {
    /// use neva::app::notification_bus::BusNotification;
    ///
    /// let notification = BusNotification::new("notifications/prompts/list_changed", None);
    /// assert_eq!(notification.method(), "notifications/prompts/list_changed");
    /// # }
    /// ```
    pub fn method(&self) -> &str {
        &self.method
    }

    /// Returns the notification's params, if it carries any.
    ///
    /// # Examples
    /// ```
    /// # #[cfg(not(feature = "legacy-spec"))] {
    /// use neva::app::notification_bus::BusNotification;
    ///
    /// let params = serde_json::json!({ "uri": "res://config" });
    /// let notification = BusNotification::new("notifications/resources/updated", Some(params));
    /// assert_eq!(notification.params().unwrap()["uri"], "res://config");
    /// # }
    /// ```
    pub fn params(&self) -> Option<&serde_json::Value> {
        self.params.as_ref()
    }

    /// Takes the notification apart, for a bus that ships the two halves
    /// separately (a channel topic plus a payload, say) rather than as one
    /// serialized value.
    ///
    /// # Examples
    /// ```
    /// # #[cfg(not(feature = "legacy-spec"))] {
    /// use neva::app::notification_bus::BusNotification;
    ///
    /// let notification = BusNotification::new("notifications/tools/list_changed", None);
    /// let (method, params) = notification.into_parts();
    ///
    /// assert_eq!(method, "notifications/tools/list_changed");
    /// assert!(params.is_none());
    /// # }
    /// ```
    pub fn into_parts(self) -> (String, Option<serde_json::Value>) {
        (self.method, self.params)
    }
}

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
/// node-local, and for a complete implementation.
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
    ///
    /// Written as `-> impl Future` rather than `async fn` only to demand the
    /// `Send` bound the server needs; an implementation writes a plain
    /// `async fn publish(&self, notification: BusNotification)`.
    fn publish(&self, notification: BusNotification) -> impl Future<Output = ()> + Send;

    /// Yields the notifications published by any instance, this one's own
    /// included.
    ///
    /// Called once per server, at startup; the server drains the stream into
    /// its local subscribers until the stream ends or the server shuts down. A
    /// stream that ends stops delivery for the rest of the process's life, so
    /// an implementation that can reconnect should do so inside the stream
    /// rather than end it.
    fn subscribe(&self) -> impl Stream<Item = BusNotification> + Send + 'static;
}

/// The `dyn`-compatible shape of [`NotificationBus`], which the server stores
/// and calls.
///
/// [`NotificationBus`] returns `impl Future` / `impl Stream` so that
/// implementing it is plain `async fn` and needs no `Pin<Box<..>>` anywhere.
/// Those are not `dyn`-compatible, and the server holds exactly one bus behind
/// an `Arc<dyn ..>`, so the boxing has to happen somewhere: it happens here,
/// once, in a blanket impl nobody outside this module ever names.
pub(crate) trait DynNotificationBus: Send + Sync {
    /// [`NotificationBus::publish`], boxed.
    fn publish(&self, notification: BusNotification) -> BoxFuture<'_, ()>;

    /// [`NotificationBus::subscribe`], boxed.
    fn subscribe(&self) -> BoxStream<'static, BusNotification>;
}

impl<T: NotificationBus> DynNotificationBus for T {
    #[inline]
    fn publish(&self, notification: BusNotification) -> BoxFuture<'_, ()> {
        Box::pin(NotificationBus::publish(self, notification))
    }

    #[inline]
    fn subscribe(&self) -> BoxStream<'static, BusNotification> {
        Box::pin(NotificationBus::subscribe(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_serializes_as_the_notification_body_it_describes() {
        let notification = BusNotification::new(
            "notifications/resources/updated",
            Some(serde_json::json!({ "uri": "res://a" })),
        );

        assert_eq!(
            serde_json::to_value(&notification).unwrap(),
            serde_json::json!({
                "method": "notifications/resources/updated",
                "params": { "uri": "res://a" }
            })
        );
    }

    #[test]
    fn it_omits_absent_params_and_reads_them_back_as_absent() {
        let notification = BusNotification::new("notifications/tools/list_changed", None);
        let wire = serde_json::to_value(&notification).unwrap();

        assert_eq!(
            wire,
            serde_json::json!({ "method": "notifications/tools/list_changed" }),
            "a notification with no params must not ship a null"
        );
        assert_eq!(
            serde_json::from_value::<BusNotification>(wire).unwrap(),
            notification
        );
    }

    /// The round trip a bus shipping JSON depends on.
    #[test]
    fn it_round_trips_through_serde() {
        let notification = BusNotification::new(
            "notifications/prompts/list_changed",
            Some(serde_json::json!({ "any": ["payload", 1, null] })),
        );

        let back: BusNotification =
            serde_json::from_str(&serde_json::to_string(&notification).unwrap()).unwrap();

        assert_eq!(back, notification);
        assert_eq!(back.into_parts().0, "notifications/prompts/list_changed");
    }

    /// The blanket impl is what lets a plain `async fn` bus be stored behind
    /// `Arc<dyn ..>`; if it stops applying, the server stops compiling in a
    /// place far from here.
    #[tokio::test]
    async fn a_plain_async_impl_is_usable_through_the_dyn_shim() {
        use futures_util::StreamExt;
        use std::sync::Arc;

        struct Echo(tokio::sync::broadcast::Sender<BusNotification>);

        impl NotificationBus for Echo {
            async fn publish(&self, notification: BusNotification) {
                let _ = self.0.send(notification);
            }

            fn subscribe(&self) -> impl Stream<Item = BusNotification> + Send + 'static {
                let rx = self.0.subscribe();
                futures_util::stream::unfold(rx, |mut rx| async move {
                    rx.recv().await.ok().map(|n| (n, rx))
                })
            }
        }

        let (tx, _) = tokio::sync::broadcast::channel(8);
        let bus: Arc<dyn DynNotificationBus> = Arc::new(Echo(tx));

        // Subscribing first: a bus is not obliged to hold anything for a
        // drain that has not started, which is what at-most-once means.
        let mut stream = bus.subscribe();
        bus.publish(BusNotification::new(
            "notifications/tools/list_changed",
            None,
        ))
        .await;

        let received = stream.next().await.expect("the publish must come back");
        assert_eq!(received.method(), "notifications/tools/list_changed");
    }
}

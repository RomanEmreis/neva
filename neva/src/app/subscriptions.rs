//! Live `subscriptions/listen` streams (MCP 2026-07-28).
//!
//! Each `subscriptions/listen` request registers one entry here for as long as
//! it is held open. Any other in-flight request can then fan a notification out
//! to the streams that asked for it -- the registry lives on the shared
//! [`McpOptions`](crate::app::options::McpOptions), so a tool handler calling
//! [`Context::add_tool`](crate::Context::add_tool) reaches every listener
//! without knowing anything about them.
//!
//! This registry is **node-local by construction**: half of every entry is a
//! handle to a socket on this process (an `mpsc::Sender` into one held-open
//! response body, an in-process cancellation token), which no other instance
//! could write into however the entry were shared. Reaching subscribers on
//! *other* instances is the job of
//! [`NotificationBus`](crate::app::notification_bus::NotificationBus), which
//! carries the notification across and lets each instance broadcast it here.

use crate::types::{
    Message, RequestId, SubscriptionFilter, SubscriptionMeta, Uri, notification::Notification,
    subscription::is_subscribable,
};
use dashmap::DashMap;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio::sync::mpsc::{OwnedPermit, Sender};
use tokio_util::sync::CancellationToken;

/// Notifications buffered per subscription before delivery starts dropping.
///
/// Only used for the transports that need a channel of their own (stdio); over
/// HTTP the subscription writes into the `POST` response sink, which is sized
/// by `sse_log_queue_capacity`.
pub(crate) const DEFAULT_SUBSCRIPTION_CAPACITY: usize = 64;

/// Server-assigned key of a registry entry.
///
/// Deliberately *not* the JSON-RPC id of the listen request: that id is chosen
/// by the peer, and every fresh neva client starts its counter at the same
/// value, so two clients on one server routinely pick the same one. Keying on
/// it would let a second client evict the first from this process-wide
/// registry, silently stop its delivery, and have either client's teardown
/// remove the other's entry.
type Key = u64;

/// One live subscription.
#[derive(Debug)]
struct Subscription {
    /// The JSON-RPC id of the `subscriptions/listen` request, as the peer chose
    /// it. Unique only within one client, so it identifies the subscription
    /// *on the wire* (it is what `_meta` is tagged with) but never keys this
    /// registry.
    id: RequestId,

    /// Transport session this subscription is bound to: the per-`POST` session
    /// id over HTTP, `None` over stdio. It is what scopes cancellation, see
    /// [`SubscriptionRegistry::cancel`].
    session_id: Option<uuid::Uuid>,

    /// The subset of the requested filter the server agreed to honor.
    accepted: SubscriptionFilter,

    /// Where this subscription's notifications go: over HTTP the `POST`
    /// response sink, over stdio a channel pumped into the transport sender.
    sink: Sender<Message>,

    /// Cancels this subscription alone, leaving the request's own cancellation
    /// token untouched -- the `subscriptions/listen` request must still be able
    /// to answer with its graceful-close result.
    token: CancellationToken,
}

/// Registry of live `subscriptions/listen` streams.
#[derive(Debug, Default, Clone)]
pub(crate) struct SubscriptionRegistry {
    entries: Arc<DashMap<Key, Subscription>>,
    next_key: Arc<AtomicU64>,
}

/// Removes a subscription from the registry when the `subscriptions/listen`
/// handler ends, whatever ends it: cancellation, disconnect or a panic.
#[derive(Debug)]
pub(crate) struct SubscriptionGuard {
    key: Key,
    registry: SubscriptionRegistry,
}

impl Drop for SubscriptionGuard {
    fn drop(&mut self) {
        self.registry.entries.remove(&self.key);
    }
}

impl SubscriptionRegistry {
    /// Queues `ack` onto the subscription's stream and registers the
    /// subscription, returning its cancellation token together with the guard
    /// that deregisters it.
    ///
    /// `session_id` is the transport session the subscription is bound to (the
    /// per-`POST` session id over HTTP, `None` over stdio).
    ///
    /// The acknowledgment is queued *here*, not by the caller, because the two
    /// steps have to be atomic against [`Self::broadcast`] and only this type
    /// can make them so. Both happen under the entry's write lock, which
    /// `broadcast` must take to read the map, so every broadcast falls entirely
    /// on one side or the other: before, when this subscription did not exist
    /// yet and nothing was owed to it, or after, when its notification queues
    /// behind an acknowledgment that is already in the stream.
    ///
    /// Doing it the obvious way instead -- queue the acknowledgment, then
    /// register -- leaves a window that no amount of care in the caller closes:
    /// the sink is drained concurrently (by the `POST` response stream over
    /// HTTP, by the pump task over stdio), so the client can be reading the
    /// acknowledgment off the wire while the entry still does not exist, and a
    /// mutation in that instant is dropped by a subscription the client has
    /// been told is live.
    ///
    /// `ack_slot` is capacity reserved for the acknowledgment before anything
    /// else could write to the body, so queueing it cannot fail and cannot be
    /// crowded out by a noisy request.
    pub(crate) fn register(
        &self,
        id: RequestId,
        session_id: Option<uuid::Uuid>,
        accepted: SubscriptionFilter,
        sink: Sender<Message>,
        ack: Message,
        ack_slot: OwnedPermit<Message>,
    ) -> (CancellationToken, SubscriptionGuard) {
        let token = CancellationToken::new();
        let key = self.next_key.fetch_add(1, Ordering::Relaxed);

        // Holds the shard's write lock for as long as it is alive, which is
        // what makes the pair below atomic.
        let slot = self.entries.entry(key);

        ack_slot.send(ack);

        slot.insert(Subscription {
            id,
            session_id,
            accepted,
            sink,
            token: token.clone(),
        });

        (
            token,
            SubscriptionGuard {
                key,
                registry: self.clone(),
            },
        )
    }

    /// Cancels the subscription a `notifications/cancelled` names, if this
    /// notification is one that may reach it.
    ///
    /// Returns whether a subscription was found -- the caller uses that to tell
    /// a cancel aimed at a subscription apart from one aimed at an ordinary
    /// in-flight request.
    ///
    /// Only subscriptions with no transport session binding can be cancelled
    /// this way, which means stdio -- where one process serves one client, so a
    /// bare request id is unambiguous, and where the spec names
    /// `notifications/cancelled` as *the* mechanism. Over HTTP the cancel
    /// arrives on its own `POST` and carries no evidence of who opened the
    /// stream, while ids collide routinely (every fresh neva client starts its
    /// counter at the same value); honoring it there would let one client end
    /// another's subscription. HTTP clients cancel by closing the stream, which
    /// the handler observes through its sink -- neva's own client does exactly
    /// that in `Subscription::cancel`.
    pub(crate) fn cancel(&self, id: &RequestId) -> bool {
        let mut found = false;
        for entry in self.entries.iter() {
            if entry.session_id.is_none() && entry.id == *id {
                entry.token.cancel();
                found = true;
            }
        }
        found
    }

    /// Returns whether no subscription is live.
    ///
    /// Read twice on the shutdown path: once to decide whether a drain is owed
    /// at all -- a server that never opened a subscription must not pay for
    /// one -- and then repeatedly, to learn when every listen handler has woken
    /// and deregistered.
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns whether any live subscription is watching `uri`.
    pub(crate) fn is_resource_subscribed(&self, uri: &Uri) -> bool {
        self.entries
            .iter()
            .any(|e| e.accepted.resource_subscriptions.contains(uri))
    }

    /// Fans `method` out to every subscription whose filter admits it, tagging
    /// each copy with its own subscription id.
    ///
    /// Returns whether `method` is a subscription-delivered notification type
    /// at all -- progress, task status and elicitation notifications stay
    /// request-scoped and are dropped here. `Context::send_notification` makes
    /// the same check before it publishes, so on that path this is a
    /// no-op; it earns its keep on the other caller, the task draining a
    /// [`NotificationBus`](crate::app::notification_bus::NotificationBus),
    /// which has no such guarantee about what an out-of-crate bus hands it.
    ///
    /// Delivery is best-effort: a subscription whose buffer is full drops the
    /// notification rather than blocking the request that produced it, exactly
    /// like the request-scoped notification sink.
    pub(crate) fn broadcast(&self, method: &str, params: Option<&serde_json::Value>) -> bool {
        if !is_subscribable(method) {
            return false;
        }

        let uri = params
            .and_then(|p| p.get("uri"))
            .and_then(|u| u.as_str())
            .map(Uri::from);

        for entry in self.entries.iter() {
            if !entry.accepted.matches(method, uri.as_ref()) {
                continue;
            }
            let notification = Notification::new(method, Some(tag(params, entry.id.clone())));
            if entry
                .sink
                .try_send(Message::Notification(notification))
                .is_err()
            {
                #[cfg(feature = "tracing")]
                tracing::warn!(
                    logger = "neva",
                    method,
                    subscription = %entry.id,
                    "dropped a notification: the subscription stream is full or closed"
                );
            }
        }
        true
    }
}

/// Stamps `_meta.io.modelcontextprotocol/subscriptionId` onto a copy of
/// `params`, which is what lets a client demultiplex several subscriptions
/// sharing one channel (stdio always does).
fn tag(params: Option<&serde_json::Value>, id: RequestId) -> serde_json::Value {
    let mut value = params.cloned().unwrap_or_else(|| serde_json::json!({}));
    let meta =
        serde_json::to_value(SubscriptionMeta::new(id)).unwrap_or_else(|_| serde_json::json!({}));

    match value.as_object_mut() {
        Some(obj) => {
            obj.insert("_meta".to_owned(), meta);
            value
        }
        // Params that are not an object cannot carry `_meta`; the tag matters
        // more than the payload shape, so rebuild around it.
        None => serde_json::json!({ "_meta": meta }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{prompt, resource, subscription::SUBSCRIPTION_ID_KEY, tool};
    use tokio::sync::mpsc::channel;

    async fn registry_with(
        id: RequestId,
        accepted: SubscriptionFilter,
    ) -> (
        SubscriptionRegistry,
        tokio::sync::mpsc::Receiver<Message>,
        SubscriptionGuard,
    ) {
        let registry = SubscriptionRegistry::default();
        let (tx, mut rx) = channel::<Message>(8);
        let (_token, guard) = register(&registry, id, None, accepted, tx, &mut rx).await;
        (registry, rx, guard)
    }

    /// Registers the way `Context::listen` does -- with the acknowledgment --
    /// then takes it off the stream, so each test below is about what the
    /// subscription carries *after* its handshake.
    ///
    /// Asserting on it here is the point: `register` queuing it is the only
    /// thing that makes "acknowledgment first" hold against a broadcast racing
    /// the registration.
    async fn register(
        registry: &SubscriptionRegistry,
        id: RequestId,
        session_id: Option<uuid::Uuid>,
        accepted: SubscriptionFilter,
        tx: Sender<Message>,
        rx: &mut tokio::sync::mpsc::Receiver<Message>,
    ) -> (CancellationToken, SubscriptionGuard) {
        let slot = tx
            .clone()
            .reserve_owned()
            .await
            .expect("the test sink must have room for the acknowledgment");
        let registered = registry.register(id, session_id, accepted, tx, ack(), slot);

        let first = rx
            .try_recv()
            .expect("register must queue the acknowledgment");
        assert_eq!(
            method_of(&first),
            crate::types::subscription::commands::ACKNOWLEDGED,
            "the acknowledgment must be the first message on the stream"
        );

        registered
    }

    fn ack() -> Message {
        Message::Notification(Notification::new(
            crate::types::subscription::commands::ACKNOWLEDGED,
            None,
        ))
    }

    fn method_of(msg: &Message) -> String {
        serde_json::to_value(msg).unwrap()["method"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[tokio::test]
    async fn it_delivers_only_subscribed_types() {
        let (registry, mut rx, _guard) = registry_with(
            RequestId::Number(1),
            SubscriptionFilter::new().with_tools_changed(),
        )
        .await;

        assert!(registry.broadcast(prompt::commands::LIST_CHANGED, None));
        assert!(registry.broadcast(tool::commands::LIST_CHANGED, None));

        let msg = rx.try_recv().expect("the tools notification is subscribed");
        assert_eq!(method_of(&msg), tool::commands::LIST_CHANGED);
        assert!(
            rx.try_recv().is_err(),
            "a type the client never requested must not be delivered"
        );
    }

    #[tokio::test]
    async fn it_tags_every_notification_with_its_subscription_id() {
        let (registry, mut rx, _guard) = registry_with(
            RequestId::String("sub-1".into()),
            SubscriptionFilter::new().with_tools_changed(),
        )
        .await;

        registry.broadcast(tool::commands::LIST_CHANGED, None);

        let msg = rx.try_recv().unwrap();
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["params"]["_meta"][SUBSCRIPTION_ID_KEY], "sub-1");
    }

    #[tokio::test]
    async fn it_keeps_resource_update_params_alongside_the_tag() {
        let (registry, mut rx, _guard) = registry_with(
            RequestId::Number(1),
            SubscriptionFilter::new().with_resource("res://a"),
        )
        .await;

        let params = serde_json::json!({ "uri": "res://a" });
        registry.broadcast(resource::commands::UPDATED, Some(&params));

        let json = serde_json::to_value(rx.try_recv().unwrap()).unwrap();
        assert_eq!(json["params"]["uri"], "res://a");
        assert_eq!(json["params"]["_meta"][SUBSCRIPTION_ID_KEY], 1);
    }

    #[tokio::test]
    async fn it_routes_resource_updates_by_uri() {
        let (registry, mut rx, _guard) = registry_with(
            RequestId::Number(1),
            SubscriptionFilter::new().with_resource("res://a"),
        )
        .await;

        let other = serde_json::json!({ "uri": "res://b" });
        registry.broadcast(resource::commands::UPDATED, Some(&other));

        assert!(
            rx.try_recv().is_err(),
            "an update for an unsubscribed URI must not be delivered"
        );
    }

    #[tokio::test]
    async fn it_reports_non_subscribable_methods() {
        let registry = SubscriptionRegistry::default();
        assert!(!registry.broadcast("notifications/progress", None));
        assert!(!registry.broadcast("notifications/tasks/status", None));
    }

    #[tokio::test]
    async fn it_fans_out_to_concurrent_subscriptions() {
        let registry = SubscriptionRegistry::default();
        let (tx1, mut rx1) = channel::<Message>(8);
        let (tx2, mut rx2) = channel::<Message>(8);

        let (_t1, _g1) = register(
            &registry,
            RequestId::Number(1),
            None,
            SubscriptionFilter::new().with_tools_changed(),
            tx1,
            &mut rx1,
        )
        .await;
        let (_t2, _g2) = register(
            &registry,
            RequestId::Number(2),
            None,
            SubscriptionFilter::new().with_prompts_changed(),
            tx2,
            &mut rx2,
        )
        .await;

        registry.broadcast(tool::commands::LIST_CHANGED, None);

        let json = serde_json::to_value(rx1.try_recv().unwrap()).unwrap();
        assert_eq!(json["params"]["_meta"][SUBSCRIPTION_ID_KEY], 1);
        assert!(rx2.try_recv().is_err());
    }

    #[tokio::test]
    async fn it_cancels_only_the_named_subscription() {
        let registry = SubscriptionRegistry::default();
        let (tx1, mut rx1) = channel::<Message>(8);
        let (tx2, mut rx2) = channel::<Message>(8);

        let (token1, _g1) = register(
            &registry,
            RequestId::Number(1),
            None,
            SubscriptionFilter::new(),
            tx1,
            &mut rx1,
        )
        .await;
        let (token2, _g2) = register(
            &registry,
            RequestId::Number(2),
            None,
            SubscriptionFilter::new(),
            tx2,
            &mut rx2,
        )
        .await;

        assert!(registry.cancel(&RequestId::Number(1)));
        assert!(token1.is_cancelled());
        assert!(!token2.is_cancelled());
        assert!(!registry.cancel(&RequestId::Number(3)));
    }

    #[tokio::test]
    async fn it_deregisters_on_guard_drop() {
        let registry = SubscriptionRegistry::default();
        let (tx, mut rx) = channel::<Message>(8);
        let (_token, guard) = register(
            &registry,
            RequestId::Number(1),
            None,
            SubscriptionFilter::new().with_tools_changed(),
            tx,
            &mut rx,
        )
        .await;

        drop(guard);

        registry.broadcast(tool::commands::LIST_CHANGED, None);
        assert!(rx.try_recv().is_err());
        assert!(!registry.cancel(&RequestId::Number(1)));
    }

    /// Two clients routinely pick the same JSON-RPC id -- every fresh neva
    /// client starts its counter at the same value -- so the registry must not
    /// be keyed on it: the second registration would evict the first, silently
    /// ending its delivery.
    #[tokio::test]
    async fn it_keeps_two_clients_that_picked_the_same_id_apart() {
        let registry = SubscriptionRegistry::default();
        let (tx_a, mut rx_a) = channel::<Message>(8);
        let (tx_b, mut rx_b) = channel::<Message>(8);

        let (_ta, _ga) = register(
            &registry,
            RequestId::Number(1),
            Some(uuid::Uuid::new_v4()),
            SubscriptionFilter::new().with_tools_changed(),
            tx_a,
            &mut rx_a,
        )
        .await;
        let (_tb, _gb) = register(
            &registry,
            RequestId::Number(1),
            Some(uuid::Uuid::new_v4()),
            SubscriptionFilter::new().with_tools_changed(),
            tx_b,
            &mut rx_b,
        )
        .await;

        registry.broadcast(tool::commands::LIST_CHANGED, None);

        // Both streams get it, and each is tagged with the id its own client
        // chose -- which here happens to be the same one.
        for rx in [&mut rx_a, &mut rx_b] {
            let json = serde_json::to_value(rx.try_recv().unwrap()).unwrap();
            assert_eq!(json["params"]["_meta"][SUBSCRIPTION_ID_KEY], 1);
        }
    }

    /// ...and one client's teardown must not deregister the other's.
    #[tokio::test]
    async fn it_does_not_deregister_a_colliding_id_on_teardown() {
        let registry = SubscriptionRegistry::default();
        let (tx_a, mut rx_a) = channel::<Message>(8);
        let (tx_b, mut rx_b) = channel::<Message>(8);

        let (_ta, guard_a) = register(
            &registry,
            RequestId::Number(1),
            Some(uuid::Uuid::new_v4()),
            SubscriptionFilter::new().with_tools_changed(),
            tx_a,
            &mut rx_a,
        )
        .await;
        let (_tb, _gb) = register(
            &registry,
            RequestId::Number(1),
            Some(uuid::Uuid::new_v4()),
            SubscriptionFilter::new().with_tools_changed(),
            tx_b,
            &mut rx_b,
        )
        .await;

        drop(guard_a);
        registry.broadcast(tool::commands::LIST_CHANGED, None);

        assert!(
            rx_b.try_recv().is_ok(),
            "the surviving subscription must still receive"
        );
    }

    /// A `notifications/cancelled` must not reach a session-bound (HTTP)
    /// subscription: it arrives on its own `POST` and says nothing about who
    /// sent it, while ids collide routinely -- honoring it would let one client
    /// silence another. Those clients cancel by closing the stream.
    #[tokio::test]
    async fn it_refuses_to_cancel_a_session_bound_subscription() {
        let registry = SubscriptionRegistry::default();
        let (tx_a, mut rx_a) = channel::<Message>(8);
        let (tx_b, mut rx_b) = channel::<Message>(8);

        let (token_a, _ga) = register(
            &registry,
            RequestId::Number(1),
            Some(uuid::Uuid::new_v4()),
            SubscriptionFilter::new(),
            tx_a,
            &mut rx_a,
        )
        .await;
        let (token_b, _gb) = register(
            &registry,
            RequestId::Number(1),
            Some(uuid::Uuid::new_v4()),
            SubscriptionFilter::new(),
            tx_b,
            &mut rx_b,
        )
        .await;

        assert!(!registry.cancel(&RequestId::Number(1)));
        assert!(!token_a.is_cancelled());
        assert!(!token_b.is_cancelled());
    }

    #[tokio::test]
    async fn it_answers_whether_a_resource_is_watched() {
        let (registry, _rx, _guard) = registry_with(
            RequestId::Number(1),
            SubscriptionFilter::new().with_resource("res://a"),
        )
        .await;

        assert!(registry.is_resource_subscribed(&Uri::from("res://a")));
        assert!(!registry.is_resource_subscribed(&Uri::from("res://b")));
    }
}

//! Live `subscriptions/listen` streams (MCP 2026-07-28).
//!
//! Each `subscriptions/listen` request registers one entry here for as long as
//! it is held open. Any other in-flight request can then fan a notification out
//! to the streams that asked for it -- the registry lives on the shared
//! [`McpOptions`](crate::app::options::McpOptions), so a tool handler calling
//! [`Context::add_tool`](crate::Context::add_tool) reaches every listener
//! without knowing anything about them.

use crate::types::{
    Message, RequestId, SubscriptionFilter, SubscriptionMeta, Uri, notification::Notification,
};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

/// Notifications buffered per subscription before delivery starts dropping.
///
/// Only used for the transports that need a channel of their own (stdio); over
/// HTTP the subscription writes into the `POST` response sink, which is sized
/// by `sse_log_queue_capacity`.
pub(crate) const DEFAULT_SUBSCRIPTION_CAPACITY: usize = 64;

/// One live subscription.
#[derive(Debug)]
struct Subscription {
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

/// Registry of live `subscriptions/listen` streams, keyed by the JSON-RPC id of
/// the request that opened each one.
#[derive(Debug, Default, Clone)]
pub(crate) struct SubscriptionRegistry {
    entries: Arc<DashMap<RequestId, Subscription>>,
}

/// Removes a subscription from the registry when the `subscriptions/listen`
/// handler ends, whatever ends it: cancellation, disconnect or a panic.
#[derive(Debug)]
pub(crate) struct SubscriptionGuard {
    id: RequestId,
    registry: SubscriptionRegistry,
}

impl Drop for SubscriptionGuard {
    fn drop(&mut self) {
        self.registry.entries.remove(&self.id);
    }
}

impl SubscriptionRegistry {
    /// Registers a subscription and returns its cancellation token together
    /// with the guard that deregisters it.
    pub(crate) fn register(
        &self,
        id: RequestId,
        accepted: SubscriptionFilter,
        sink: Sender<Message>,
    ) -> (CancellationToken, SubscriptionGuard) {
        let token = CancellationToken::new();
        self.entries.insert(
            id.clone(),
            Subscription {
                accepted,
                sink,
                token: token.clone(),
            },
        );
        (
            token,
            SubscriptionGuard {
                id,
                registry: self.clone(),
            },
        )
    }

    /// Cancels the subscription opened by `id`, if there is one.
    ///
    /// Returns whether a subscription was found -- the caller uses that to tell
    /// a `notifications/cancelled` aimed at a subscription apart from one aimed
    /// at an ordinary in-flight request.
    pub(crate) fn cancel(&self, id: &RequestId) -> bool {
        match self.entries.get(id) {
            Some(entry) => {
                entry.token.cancel();
                true
            }
            None => false,
        }
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
    /// at all, so the caller can tell "nobody is listening" from "this
    /// notification never travels on a subscription" (progress, task status and
    /// elicitation notifications stay request-scoped).
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
            let notification = Notification::new(method, Some(tag(params, entry.key().clone())));
            if entry
                .sink
                .try_send(Message::Notification(notification))
                .is_err()
            {
                #[cfg(feature = "tracing")]
                tracing::warn!(
                    logger = "neva",
                    method,
                    subscription = %entry.key(),
                    "dropped a notification: the subscription stream is full or closed"
                );
            }
        }
        true
    }
}

/// Whether `method` is one of the notification types a `subscriptions/listen`
/// filter can opt in to.
fn is_subscribable(method: &str) -> bool {
    use crate::types::{prompt, resource, tool};

    matches!(
        method,
        tool::commands::LIST_CHANGED
            | prompt::commands::LIST_CHANGED
            | resource::commands::LIST_CHANGED
            | resource::commands::UPDATED
    )
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

    fn registry_with(
        id: RequestId,
        accepted: SubscriptionFilter,
    ) -> (
        SubscriptionRegistry,
        tokio::sync::mpsc::Receiver<Message>,
        SubscriptionGuard,
    ) {
        let registry = SubscriptionRegistry::default();
        let (tx, rx) = channel::<Message>(8);
        let (_token, guard) = registry.register(id, accepted, tx);
        (registry, rx, guard)
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
        );

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
        );

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
        );

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
        );

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

        let (_t1, _g1) = registry.register(
            RequestId::Number(1),
            SubscriptionFilter::new().with_tools_changed(),
            tx1,
        );
        let (_t2, _g2) = registry.register(
            RequestId::Number(2),
            SubscriptionFilter::new().with_prompts_changed(),
            tx2,
        );

        registry.broadcast(tool::commands::LIST_CHANGED, None);

        let json = serde_json::to_value(rx1.try_recv().unwrap()).unwrap();
        assert_eq!(json["params"]["_meta"][SUBSCRIPTION_ID_KEY], 1);
        assert!(rx2.try_recv().is_err());
    }

    #[tokio::test]
    async fn it_cancels_only_the_named_subscription() {
        let registry = SubscriptionRegistry::default();
        let (tx1, _rx1) = channel::<Message>(8);
        let (tx2, _rx2) = channel::<Message>(8);

        let (token1, _g1) = registry.register(RequestId::Number(1), SubscriptionFilter::new(), tx1);
        let (token2, _g2) = registry.register(RequestId::Number(2), SubscriptionFilter::new(), tx2);

        assert!(registry.cancel(&RequestId::Number(1)));
        assert!(token1.is_cancelled());
        assert!(!token2.is_cancelled());
        assert!(!registry.cancel(&RequestId::Number(3)));
    }

    #[tokio::test]
    async fn it_deregisters_on_guard_drop() {
        let registry = SubscriptionRegistry::default();
        let (tx, mut rx) = channel::<Message>(8);
        let (_token, guard) = registry.register(
            RequestId::Number(1),
            SubscriptionFilter::new().with_tools_changed(),
            tx,
        );

        drop(guard);

        registry.broadcast(tool::commands::LIST_CHANGED, None);
        assert!(rx.try_recv().is_err());
        assert!(!registry.cancel(&RequestId::Number(1)));
    }

    #[tokio::test]
    async fn it_answers_whether_a_resource_is_watched() {
        let (registry, _rx, _guard) = registry_with(
            RequestId::Number(1),
            SubscriptionFilter::new().with_resource("res://a"),
        );

        assert!(registry.is_resource_subscribed(&Uri::from("res://a")));
        assert!(!registry.is_resource_subscribed(&Uri::from("res://b")));
    }
}

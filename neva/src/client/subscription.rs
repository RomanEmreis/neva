//! Client-side handle for a live `subscriptions/listen` stream (MCP 2026-07-28).

use crate::error::{Error, ErrorCode};
use crate::shared::PendingResponse;
use crate::transport::{Sender as _, TransportProtoSender};
use crate::types::{
    RequestId, SubscriptionFilter, SubscriptionsListenResult,
    notification::{CancelledNotificationParams, Notification},
};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Waiters for `notifications/subscriptions/acknowledged`, keyed by the id of
/// the `subscriptions/listen` request each one belongs to.
///
/// The acknowledgment is a notification, so it arrives on the receive loop
/// rather than as the request's reply -- the reply only comes when the
/// subscription ends. This is what lets [`Client::listen`](crate::Client::listen)
/// return once the server has confirmed the filter.
pub(super) type AckWaiters = Arc<DashMap<RequestId, oneshot::Sender<SubscriptionFilter>>>;

/// How a subscription stream ended.
#[derive(Debug)]
pub enum SubscriptionEnd {
    /// The server answered the `subscriptions/listen` request with its
    /// graceful-close result.
    Graceful(SubscriptionsListenResult),

    /// The stream went away without a final result -- a dropped connection, a
    /// timeout, or a server that died. Subscriptions are not resumable, so a
    /// client that wants to keep listening re-sends `subscriptions/listen`.
    Abrupt,

    /// This client ended it via [`Subscription::cancel`].
    ///
    /// Over HTTP that closes the stream, which is the spec's cancellation
    /// mechanism there -- so no final result comes back, and none is expected.
    Cancelled,
}

/// A live `subscriptions/listen` stream.
///
/// Notifications delivered on the stream are dispatched to the handlers
/// registered with [`Client::subscribe`](crate::Client::subscribe) and its
/// helpers ([`on_tools_changed`](crate::Client::on_tools_changed) and
/// friends), so this handle is about the subscription's *lifecycle*: what the
/// server agreed to send, cancelling it, and observing how it ended.
///
/// # Examples
/// ```no_run
/// # #[cfg(all(feature = "client", not(feature = "legacy-spec")))] {
/// use neva::{Client, error::Error, types::SubscriptionFilter};
///
/// #[tokio::main]
/// async fn main() -> Result<(), Error> {
///     let mut client = Client::new();
///     client.on_tools_changed(|_| async { println!("tools changed"); });
///     client.connect().await?;
///
///     let mut subscription = client
///         .listen(SubscriptionFilter::new().with_tools_changed())
///         .await?;
///
///     // ... work ...
///
///     subscription.cancel().await?;
///     Ok(())
/// }
/// # }
/// ```
pub struct Subscription {
    id: RequestId,
    requested: SubscriptionFilter,
    acknowledged: SubscriptionFilter,
    response: oneshot::Receiver<PendingResponse>,
    sender: TransportProtoSender,
    /// This client cancelled it -- what [`Subscription::closed`] reports.
    cancelled: bool,
    /// Teardown is already done or unnecessary, so [`Drop`] has nothing to do.
    settled: bool,
}

impl std::fmt::Debug for Subscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscription")
            .field("id", &self.id)
            .field("requested", &self.requested)
            .field("acknowledged", &self.acknowledged)
            .finish_non_exhaustive()
    }
}

impl Subscription {
    /// Creates a handle for an established subscription.
    pub(super) fn new(
        id: RequestId,
        requested: SubscriptionFilter,
        acknowledged: SubscriptionFilter,
        response: oneshot::Receiver<PendingResponse>,
        sender: TransportProtoSender,
    ) -> Self {
        Self {
            id,
            requested,
            acknowledged,
            response,
            sender,
            cancelled: false,
            settled: false,
        }
    }

    /// Returns the subscription id -- the JSON-RPC id of the
    /// `subscriptions/listen` request, which every message on the stream
    /// carries in `_meta`.
    #[inline]
    pub fn id(&self) -> &RequestId {
        &self.id
    }

    /// Returns the filter this client asked for.
    #[inline]
    pub fn requested(&self) -> &SubscriptionFilter {
        &self.requested
    }

    /// Returns the subset the server agreed to honor.
    ///
    /// Types the server does not support are omitted, so a client that cares
    /// should compare this against [`Self::requested`].
    #[inline]
    pub fn acknowledged(&self) -> &SubscriptionFilter {
        &self.acknowledged
    }

    /// Returns whether the server honored every requested notification type.
    #[inline]
    pub fn is_fully_honored(&self) -> bool {
        self.requested.is_subset_of(&self.acknowledged)
    }

    /// Cancels the subscription.
    ///
    /// Sends `notifications/cancelled` for the `subscriptions/listen` request,
    /// which ends the subscription server-side -- directly over stdio, and over
    /// HTTP by way of the transport closing the listen response body. Await
    /// [`Self::closed`] afterwards to confirm how it ended.
    pub async fn cancel(&mut self) -> Result<(), Error> {
        self.cancelled = true;
        self.settled = true;
        self.sender.send(cancelled(&self.id).into()).await
    }

    /// Waits for the subscription to end and reports how.
    pub async fn closed(mut self) -> SubscriptionEnd {
        // A cancel this client issued needs no waiting: over HTTP it closed the
        // stream, so the peer has nowhere to send a final result and awaiting
        // one would hang until the request timeout.
        if self.cancelled {
            return SubscriptionEnd::Cancelled;
        }

        let end = match (&mut self.response).await {
            Ok(PendingResponse::Response(resp)) => match resp.into_result() {
                Ok(result) => SubscriptionEnd::Graceful(result),
                Err(_) => SubscriptionEnd::Abrupt,
            },
            _ => SubscriptionEnd::Abrupt,
        };
        // However it ended, it ended: the peer is not streaming any more, so
        // the `Drop` below has nothing left to cancel.
        self.settled = true;
        end
    }
}

/// Dropping the handle ends the subscription.
///
/// Without this, letting a `Subscription` fall out of scope would leave the
/// peer streaming into a client that has no way left to stop it: over HTTP the
/// transport task keeps draining the response body, over stdio the server keeps
/// the entry registered, and either way the notifications go on reaching the
/// handlers registered with [`Client::subscribe`](crate::Client::subscribe).
///
/// Best-effort by necessity -- `Drop` cannot await, so the cancellation is
/// handed to the runtime. Outside a runtime there is nothing to hand it to and
/// the subscription ends with the connection instead; call
/// [`Self::cancel`] when the ending has to be observable.
impl Drop for Subscription {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };

        let mut sender = self.sender.clone();
        let notification = cancelled(&self.id);

        runtime.spawn(async move {
            let _ = sender.send(notification.into()).await;
        });
    }
}

/// Builds the `notifications/cancelled` that ends a subscription.
///
/// Over stdio this is the mechanism the spec names. Over HTTP the spec has the
/// client close the stream instead -- so there the transport reads this same
/// notification as its cue to drop the listen response body, and the server
/// learns of the cancellation from the close rather than from the message.
pub(super) fn cancelled(id: &RequestId) -> Notification {
    let params = CancelledNotificationParams {
        request_id: id.clone(),
        reason: Some("subscription cancelled by the client".into()),
    };

    Notification::new(
        crate::types::notification::commands::CANCELLED,
        serde_json::to_value(params).ok(),
    )
}

/// Reads the acknowledged filter and its subscription id out of a
/// `notifications/subscriptions/acknowledged` payload.
pub(super) fn parse_ack(
    notification: &Notification,
) -> Result<(RequestId, SubscriptionFilter), Error> {
    let params = notification
        .params
        .clone()
        .ok_or_else(|| Error::new(ErrorCode::InvalidParams, "Acknowledgment has no params"))?;

    serde_json::from_value::<crate::types::SubscriptionsAcknowledgedNotificationParams>(params)
        .map(|p| (p.meta.subscription_id, p.notifications))
        .map_err(|e| Error::new(ErrorCode::InvalidParams, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Response, SUBSCRIPTION_ID_KEY};

    fn ack(id: serde_json::Value, filter: serde_json::Value) -> Notification {
        Notification::new(
            crate::types::subscription::commands::ACKNOWLEDGED,
            Some(serde_json::json!({
                "notifications": filter,
                "_meta": { SUBSCRIPTION_ID_KEY: id },
            })),
        )
    }

    #[test]
    fn it_parses_an_acknowledgment() {
        let notification = ack(
            serde_json::json!(7),
            serde_json::json!({ "toolsListChanged": true }),
        );

        let (id, filter) = parse_ack(&notification).unwrap();

        assert_eq!(id, RequestId::Number(7));
        assert!(filter.tools_list_changed);
        assert!(!filter.prompts_list_changed);
    }

    #[test]
    fn it_rejects_an_acknowledgment_without_a_subscription_id() {
        let notification = Notification::new(
            crate::types::subscription::commands::ACKNOWLEDGED,
            Some(serde_json::json!({ "notifications": {} })),
        );

        assert!(parse_ack(&notification).is_err());
    }

    #[tokio::test]
    async fn it_reports_a_graceful_close() {
        let (tx, rx) = oneshot::channel();
        let subscription = Subscription::new(
            RequestId::Number(1),
            SubscriptionFilter::new().with_tools_changed(),
            SubscriptionFilter::new().with_tools_changed(),
            rx,
            TransportProtoSender::None,
        );

        let result = serde_json::json!({ "_meta": { SUBSCRIPTION_ID_KEY: 1 } });
        tx.send(PendingResponse::Response(Response::success(
            RequestId::Number(1),
            result,
        )))
        .unwrap();

        let end = subscription.closed().await;
        assert!(matches!(end, SubscriptionEnd::Graceful(r)
            if r.meta.subscription_id == RequestId::Number(1)));
    }

    #[tokio::test]
    async fn it_reports_an_abrupt_close_when_the_stream_drops() {
        let (tx, rx) = oneshot::channel();
        let subscription = Subscription::new(
            RequestId::Number(1),
            SubscriptionFilter::new(),
            SubscriptionFilter::new(),
            rx,
            TransportProtoSender::None,
        );

        drop(tx);

        assert!(matches!(
            subscription.closed().await,
            SubscriptionEnd::Abrupt
        ));
    }

    #[test]
    fn it_reports_a_narrowed_acknowledgment() {
        let (_tx, rx) = oneshot::channel();
        let subscription = Subscription::new(
            RequestId::Number(1),
            SubscriptionFilter::new()
                .with_tools_changed()
                .with_prompts_changed(),
            SubscriptionFilter::new().with_tools_changed(),
            rx,
            TransportProtoSender::None,
        );

        assert!(!subscription.is_fully_honored());
    }
}

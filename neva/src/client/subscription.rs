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

/// Where each subscription this client opened stands, keyed by its
/// subscription id.
///
/// The acknowledgment is a promise about what the stream will carry, and a
/// noncompliant peer can break it in either direction: by sending something
/// outside the accepted filter after acknowledging correctly, or by sending
/// anything at all before acknowledging. Notifications go to the client's
/// global handlers, so nothing downstream would notice; the receive loop checks
/// each tagged notification against this instead.
pub(super) type SubscriptionStates = Arc<DashMap<RequestId, SubscriptionState>>;

/// How far along a subscription's handshake is, and what it may deliver.
#[derive(Debug, Clone)]
pub(super) enum SubscriptionState {
    /// The `subscriptions/listen` request is out and nothing has come back.
    ///
    /// Nothing may be delivered yet: the acknowledgment is required to be the
    /// first message on the stream, and this subscription may still be
    /// rejected outright or never acknowledged at all -- in which case
    /// [`Client::listen`](crate::Client::listen) reports a failure that the
    /// user's handlers would already have seen events from.
    ///
    /// Carries the *requested* filter, which the acknowledgment narrows.
    Pending(SubscriptionFilter),

    /// Acknowledged: what the subscription is allowed to carry from here on.
    Established(SubscriptionFilter),
}

impl SubscriptionState {
    /// The accepted filter, once there is one.
    pub(super) fn established(&self) -> Option<&SubscriptionFilter> {
        match self {
            Self::Established(filter) => Some(filter),
            Self::Pending(_) => None,
        }
    }

    /// Narrows the requested filter to what the peer acknowledged, and marks
    /// the subscription established.
    ///
    /// Intersecting rather than replacing keeps a peer that acknowledges *more*
    /// than was asked from widening what this stream may deliver in the window
    /// before `listen` rejects it outright. A second acknowledgment for a
    /// subscription that already has one changes nothing: the handshake happens
    /// once.
    pub(super) fn acknowledge(&mut self, acknowledged: &SubscriptionFilter) {
        if let Self::Pending(requested) = self {
            *self = Self::Established(requested.intersection(acknowledged));
        }
    }
}

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
    /// Releases the request slot a cancelled subscription will never answer.
    release: SubscriptionRelease,
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
        release: SubscriptionRelease,
    ) -> Self {
        Self {
            id,
            requested,
            acknowledged,
            response,
            sender,
            release,
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
    ///
    /// # Errors
    /// Returns [`Error`] if the notification cannot be sent -- the transport is
    /// already gone, most likely because the client disconnected. The
    /// subscription is over either way, but it ended with the connection rather
    /// than by this call, and [`Self::closed`] reports
    /// [`SubscriptionEnd::Abrupt`] accordingly.
    pub async fn cancel(&mut self) -> Result<(), Error> {
        let sent = self.sender.send(cancelled(&self.id).into()).await;

        // Settled either way: there is no second attempt worth making on a
        // transport that just refused the first.
        self.settled = true;
        // Cancelled only if the notification actually went out. A cancel that
        // never reached the wire ended nothing -- the connection did -- and
        // reporting `Cancelled` for it would tell callers a deliberate stop
        // happened where a connection loss did, which is exactly the difference
        // reconnect logic keys on.
        self.cancelled = sent.is_ok();

        // No terminal response is coming for a stream this client closed, so the
        // request slot has to go -- otherwise a client that opens and cancels
        // subscriptions in a loop grows the pending queue by one entry per
        // cycle, and these slots carry no TTL to expire them. Dropping the slot
        // also closes the receiver `closed()` awaits, which is what resolves it
        // to `Abrupt` when the send failed.
        self.release.release(&self.id);

        sent
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
        self.release.release(&self.id);
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
        self.release.release(&self.id);

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

/// Frees the per-subscription bookkeeping a cancelled stream leaves behind: its
/// request slot, its acknowledgment waiter, and its delivery filter.
///
/// Held by the [`Subscription`] so the handle can clean up on cancel or drop
/// without borrowing the client it came from.
#[derive(Clone)]
pub(super) struct SubscriptionRelease {
    pending: crate::shared::RequestQueue,
    ack_waiters: AckWaiters,
    filters: SubscriptionStates,
}

impl SubscriptionRelease {
    pub(super) fn new(
        pending: crate::shared::RequestQueue,
        ack_waiters: AckWaiters,
        filters: SubscriptionStates,
    ) -> Self {
        Self {
            pending,
            ack_waiters,
            filters,
        }
    }

    pub(super) fn release(&self, id: &RequestId) {
        let _ = self.pending.pop(id);
        self.ack_waiters.remove(id);
        self.filters.remove(id);
    }
}

impl std::fmt::Debug for SubscriptionRelease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubscriptionRelease")
            .finish_non_exhaustive()
    }
}

/// Undoes a `subscriptions/listen` that was sent but never became a
/// [`Subscription`].
///
/// Between the send and the returned handle there is bookkeeping only an
/// explicit ending releases -- the acknowledgment waiter, the accepted filter,
/// and a request slot that carries no TTL -- and a peer already streaming into
/// it. The error paths in [`Client::listen`](crate::Client::listen) can await
/// that cleanup, but the caller dropping the whole `listen` future (an outer
/// `tokio::time::timeout`, a lost `select!` branch) runs none of them: the
/// future simply stops existing at its next suspension point. This guard is
/// what still runs then.
///
/// Armed from the moment the request goes out until ownership passes to the
/// handle ([`Self::disarm`]) or the establishment gives up ([`Self::abandon`]).
pub(super) struct EstablishmentGuard {
    id: RequestId,
    release: SubscriptionRelease,
    sender: TransportProtoSender,
    armed: bool,
}

impl EstablishmentGuard {
    pub(super) fn new(
        id: RequestId,
        release: SubscriptionRelease,
        sender: TransportProtoSender,
    ) -> Self {
        Self {
            id,
            release,
            sender,
            armed: true,
        }
    }

    /// The stream is now the returned [`Subscription`]'s to end.
    pub(super) fn disarm(mut self) {
        self.armed = false;
    }

    /// Ends an establishment that failed, awaiting the cancellation rather than
    /// handing it to the runtime the way [`Drop`] must.
    pub(super) async fn abandon(mut self) {
        self.armed = false;
        self.release.release(&self.id);
        let _ = self.sender.send(cancelled(&self.id).into()).await;
    }
}

/// Best-effort by necessity, exactly as for [`Subscription`]: `Drop` cannot
/// await, so the cancellation goes to the runtime. Outside a runtime there is
/// nothing to hand it to and the subscription ends with the connection instead.
impl Drop for EstablishmentGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }

        self.release.release(&self.id);

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

impl std::fmt::Debug for EstablishmentGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EstablishmentGuard")
            .field("id", &self.id)
            .field("armed", &self.armed)
            .finish_non_exhaustive()
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

    fn release() -> SubscriptionRelease {
        SubscriptionRelease::new(
            crate::shared::RequestQueue::new(std::time::Duration::from_secs(5)),
            Default::default(),
            Default::default(),
        )
    }

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
            release(),
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
            release(),
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
            release(),
        );

        assert!(!subscription.is_fully_honored());
    }
}

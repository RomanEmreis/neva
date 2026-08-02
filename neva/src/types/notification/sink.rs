//! Per-request notification sinks for the MCP 2026-07-28 transport.
//!
//! The 2026-07-28 transport has no session-scoped `GET`/SSE stream, so every
//! server->client notification travels on a `POST` response body: either the
//! originating request's own stream (request-scoped `notifications/message` and
//! `notifications/progress`) or the long-lived `subscriptions/listen` stream.
//! Both are the same mechanism -- a channel registered before dispatch and
//! drained into the response body -- keyed by the per-`POST` session id, which
//! is minted fresh for each `POST` and carried on every message of that request.
//!
//! This module deliberately sits outside the `tracing`-gated
//! [`fmt`](super::fmt) layer: subscriptions must work in a build without
//! `tracing`, and the layer is only one of the sinks' writers.

use crate::types::Message;
use std::sync::LazyLock;
use tokio::sync::mpsc::{OwnedPermit, Sender};

/// One registered sink: the `POST` body's writing end, plus the slot held for a
/// subscription's acknowledgment.
pub(crate) struct RequestSink {
    /// Where the body's messages go.
    tx: Sender<Message>,

    /// A capacity slot reserved for `notifications/subscriptions/acknowledged`,
    /// taken by the handler when it opens the subscription.
    ///
    /// The body is bounded and shared: middleware wrapped around the handler
    /// logs into it before `Context::listen` ever runs, and enough of that --
    /// easy with a small `with_sse_log_queue` -- would leave no room for the
    /// acknowledgment, failing a subscription that is otherwise perfectly
    /// valid. Reserving the slot up front, before any of that can run, is what
    /// makes the acknowledgment independent of how noisy the request is.
    // Only the HTTP server reserves and hands out this slot; a client-only
    // build has the map (the layer reads it) but nothing that spends one.
    #[cfg_attr(not(feature = "http-server"), allow(dead_code))]
    ack: Option<OwnedPermit<Message>>,
}

/// Live per-request notification sinks, keyed by the per-`POST` session id.
///
/// Only the HTTP server registers sinks; the notification layer (which may run
/// client-side too) merely reads the map, so the writers below are gated on
/// `http-server` while the map itself is not.
pub(crate) static REQUEST_NOTIFICATIONS: LazyLock<dashmap::DashMap<uuid::Uuid, RequestSink>> =
    LazyLock::new(dashmap::DashMap::new);

impl RequestSink {
    /// Queues a message on the body, dropping it if the buffer is full.
    // Its only caller is the `tracing` layer; without the feature nothing
    // produces request-scoped notifications to queue here.
    #[cfg_attr(not(feature = "tracing"), allow(dead_code))]
    #[inline]
    pub(crate) fn try_send(&self, msg: Message) -> Result<(), ()> {
        self.tx.try_send(msg).map_err(|_| ())
    }
}

/// Registers a notification sink for `id` (the per-`POST` session id). Returns
/// the receiving end for the `POST` response stream to drain.
///
/// `reserve_ack` marks a body that carries a `subscriptions/listen`: it gets one
/// slot beyond `capacity`, reserved then and there for the acknowledgment, so
/// the configured log capacity stays whole and the acknowledgment cannot be
/// crowded out of it.
// Also compiled for the layer's own tests, which register a sink to route into
// without an HTTP server to do it for them.
#[cfg(any(feature = "http-server", all(test, feature = "tracing")))]
pub(crate) async fn register(
    id: uuid::Uuid,
    capacity: usize,
    reserve_ack: bool,
) -> tokio::sync::mpsc::Receiver<Message> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Message>(capacity + usize::from(reserve_ack));
    // Immediate on a channel nothing has written to yet.
    let ack = match reserve_ack {
        true => tx.clone().reserve_owned().await.ok(),
        false => None,
    };
    REQUEST_NOTIFICATIONS.insert(id, RequestSink { tx, ack });
    rx
}

/// Takes the reserved acknowledgment slot for `id`, if this body has one and
/// nobody has taken it yet.
#[cfg(feature = "http-server")]
pub(crate) fn take_ack_permit(id: &uuid::Uuid) -> Option<OwnedPermit<Message>> {
    REQUEST_NOTIFICATIONS
        .get_mut(id)
        .and_then(|mut s| s.ack.take())
}

/// Removes the notification sink for `id`.
#[cfg(any(feature = "http-server", all(test, feature = "tracing")))]
pub(crate) fn unregister(id: &uuid::Uuid) {
    REQUEST_NOTIFICATIONS.remove(id);
}

/// Returns a clone of the sink registered for `id`, if the `POST` response
/// stream is still open.
///
/// A `subscriptions/listen` handler holds onto this sender for the life of the
/// subscription: it is both where the subscription's notifications go and --
/// via [`Sender::closed`] -- how the handler learns the client disconnected.
#[cfg(feature = "http-server")]
pub(crate) fn get(id: &uuid::Uuid) -> Option<Sender<Message>> {
    REQUEST_NOTIFICATIONS.get(id).map(|s| s.tx.clone())
}

#[cfg(all(test, feature = "http-server"))]
mod tests {
    use super::*;
    use crate::types::notification::Notification;

    #[tokio::test]
    async fn it_routes_to_a_registered_sink() {
        let id = uuid::Uuid::new_v4();
        let mut rx = register(id, 4, false).await;

        let sink = get(&id).expect("sink should be registered");
        sink.send(Message::Notification(Notification::new("test", None)))
            .await
            .unwrap();

        assert!(rx.recv().await.is_some());
        unregister(&id);
        assert!(get(&id).is_none());
    }

    #[tokio::test]
    async fn it_reports_a_dropped_receiver_as_closed() {
        // The disconnect signal a long-lived subscription waits on: the HTTP
        // response stream drops its receiver when the client goes away.
        let id = uuid::Uuid::new_v4();
        let rx = register(id, 4, false).await;

        let sink = get(&id).expect("sink should be registered");
        drop(rx);

        sink.closed().await;
        unregister(&id);
    }
}

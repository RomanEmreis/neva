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
use tokio::sync::mpsc::Sender;

/// Live per-request notification sinks, keyed by the per-`POST` session id.
///
/// Only the HTTP server registers sinks; the notification layer (which may run
/// client-side too) merely reads the map, so the writers below are gated on
/// `http-server` while the map itself is not.
pub(crate) static REQUEST_NOTIFICATIONS: LazyLock<dashmap::DashMap<uuid::Uuid, Sender<Message>>> =
    LazyLock::new(dashmap::DashMap::new);

/// Registers a notification sink for `id` (the per-`POST` session id). Returns
/// the receiving end for the `POST` response stream to drain.
#[cfg(feature = "http-server")]
pub(crate) fn register(id: uuid::Uuid, capacity: usize) -> tokio::sync::mpsc::Receiver<Message> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Message>(capacity);
    REQUEST_NOTIFICATIONS.insert(id, tx);
    rx
}

/// Removes the notification sink for `id`.
#[cfg(feature = "http-server")]
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
    REQUEST_NOTIFICATIONS.get(id).map(|s| s.clone())
}

#[cfg(all(test, feature = "http-server"))]
mod tests {
    use super::*;
    use crate::types::notification::Notification;

    #[tokio::test]
    async fn it_routes_to_a_registered_sink() {
        let id = uuid::Uuid::new_v4();
        let mut rx = register(id, 4);

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
        let rx = register(id, 4);

        let sink = get(&id).expect("sink should be registered");
        drop(rx);

        sink.closed().await;
        unregister(&id);
    }
}

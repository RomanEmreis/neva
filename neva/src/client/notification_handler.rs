//! Utilities for handling notifications from server

use crate::types::notification::Notification;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, PoisonError, RwLock};

/// Represents a notification handler function
pub(crate) type NotificationsHandlerFunc =
    Arc<dyn Fn(Notification) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> + Send + Sync>;

/// Represents a notification handler
///
/// The lock is held only for a map insert, removal or lookup, and never across
/// an `.await`: [`Self::notify`] clones the handler out before it runs it. A
/// synchronous lock does that from any runtime flavor, where taking an async
/// lock from the synchronous `subscribe` meant `block_in_place`, which panics
/// on a `current_thread` runtime.
#[derive(Default)]
pub(super) struct NotificationsHandler {
    handlers: RwLock<HashMap<String, NotificationsHandlerFunc>>,
}

impl NotificationsHandler {
    /// Subscribes to the `event` with the `handler`
    pub(super) fn subscribe<E, F, R>(&self, event: E, handler: F)
    where
        E: Into<String>,
        F: Fn(Notification) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = ()> + Send,
    {
        let handler: NotificationsHandlerFunc = Arc::new(move |params| {
            let handler = handler.clone();
            Box::pin(async move {
                handler(params).await;
            })
        });
        self.handlers
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(event.into(), handler);
    }

    /// Unsubscribes from the `event`
    pub(super) fn unsubscribe(&self, event: impl AsRef<str>) {
        self.handlers
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(event.as_ref());
    }

    /// Calls an appropriate notifications handler
    pub(super) async fn notify(&self, notification: Notification) {
        let handler = self
            .handlers
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&notification.method)
            .cloned();
        if let Some(handler) = handler {
            handler(notification).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn notification(method: &str) -> Notification {
        Notification::new(method, None)
    }

    /// The `current_thread` runtime is `#[tokio::main(flavor = "current_thread")]`
    /// and every `#[tokio::test]`: subscribing there must not panic (#139).
    #[tokio::test(flavor = "current_thread")]
    async fn handlers_come_and_go_on_a_current_thread_runtime() {
        let handler = NotificationsHandler::default();
        let calls = Arc::new(AtomicUsize::new(0));

        let counted = calls.clone();
        handler.subscribe("notifications/tools/list_changed", move |_| {
            let counted = counted.clone();
            async move {
                counted.fetch_add(1, Ordering::SeqCst);
            }
        });

        handler
            .notify(notification("notifications/tools/list_changed"))
            .await;
        handler
            .notify(notification("notifications/prompts/list_changed"))
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        handler.unsubscribe("notifications/tools/list_changed");
        handler
            .notify(notification("notifications/tools/list_changed"))
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "unsubscribed");
    }

    /// The case from #139, through the public API.
    #[tokio::test(flavor = "current_thread")]
    async fn a_client_subscribes_on_a_current_thread_runtime() {
        let mut client = crate::Client::new();
        client.subscribe("notifications/tools/list_changed", |_| async {});
        client.unsubscribe("notifications/tools/list_changed");
    }
}

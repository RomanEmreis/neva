//! Utilities for handling notifications from server

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::types::notification::Notification;

/// Represents a notification handler function
pub(crate) type NotificationsHandlerFunc = Arc<
    dyn Fn(Notification) -> Pin<
        Box<dyn Future<Output = ()> + Send + 'static>,
    >
    + Send
    + Sync
>;

/// Represents a notification handler
#[derive(Default)]
pub(super) struct NotificationsHandler {
    handlers: RwLock<HashMap<String, NotificationsHandlerFunc>>
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
            Box::pin(async move { handler(params).await; })
        });
        tokio::task::block_in_place(|| {
            self.handlers
                .blocking_write()
                .insert(event.into(), handler);            
        });
    }
    
    /// Unsubscribes from the `event`
    pub(super) fn unsubscribe(&self, event: impl AsRef<str>) {
        tokio::task::block_in_place(|| {
            self.handlers
                .blocking_write()
                .remove(event.as_ref());
        });
    }
    
    /// Calls an appropriate notifications handler
    pub(super) async fn notify(&self, notification: Notification) {
        let guard = self.handlers.read().await;
        if let Some(handler) = guard.get(&notification.method).cloned() {
            drop(guard);
            handler(notification).await;
        }
    }
}
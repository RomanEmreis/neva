//! Additional helper methods for [`Client`] for variety notification subscription

use super::Client;
use crate::types::notification::Notification;
use std::future::Future;

impl Client {
    /// Maps a `handler` to the `notifications/resources/updated` event
    pub fn on_resource_changed<F, R>(&mut self, handler: F)
    where
        F: Fn(Notification) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = ()> + Send,
    {
        assert!(
            self.is_resource_subscription_supported(),
            "Server does not support resource subscriptions"
        );

        self.subscribe(crate::types::resource::commands::UPDATED, handler);
    }

    /// Maps a `handler` to the `notifications/resources/list_changed` event
    pub fn on_resources_changed<F, R>(&mut self, handler: F)
    where
        F: Fn(Notification) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = ()> + Send,
    {
        assert!(
            self.is_resource_list_changed_supported(),
            "Server does not support resource list changed events"
        );

        self.subscribe(crate::types::resource::commands::LIST_CHANGED, handler);
    }

    /// Maps a `handler` to the `notifications/tools/list_changed` event
    pub fn on_tools_changed<F, R>(&mut self, handler: F)
    where
        F: Fn(Notification) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = ()> + Send,
    {
        assert!(
            self.is_tools_list_changed_supported(),
            "Server does not support tools list changed events"
        );

        self.subscribe(crate::types::tool::commands::LIST_CHANGED, handler);
    }

    /// Maps a `handler` to the `notifications/prompts/list_changed` event
    pub fn on_prompts_changed<F, R>(&mut self, handler: F)
    where
        F: Fn(Notification) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = ()> + Send,
    {
        assert!(
            self.is_prompts_list_changed_supported(),
            "Server does not support prompts list changed events"
        );

        self.subscribe(crate::types::prompt::commands::LIST_CHANGED, handler);
    }

    /// Maps a `handler` to the `notifications/elicitation/completed` event
    #[cfg(feature = "legacy-spec")]
    pub fn on_elicitation_completed<F, R>(&mut self, handler: F)
    where
        F: Fn(Notification) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = ()> + Send,
    {
        assert!(
            self.is_elicitation_supported(),
            "Client does not support elicitation. You may configure it with `Client::with_options(|opt| opt.with_elicitation())` method."
        );

        self.subscribe(crate::types::elicitation::commands::COMPLETE, handler);
    }

    /// Maps a `handler` to the `notifications/tasks/status` event
    #[cfg(feature = "tasks")]
    pub fn on_task_status<F, R>(&mut self, handler: F)
    where
        F: Fn(Notification) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = ()> + Send,
    {
        self.ensure_tasks_supported();

        self.subscribe(crate::types::task::commands::STATUS, handler);
    }
}

impl Client {
    /// Maps the `handler` to a specific `event`
    pub fn subscribe<E, F, R>(&mut self, event: E, handler: F)
    where
        E: Into<String>,
        F: Fn(Notification) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = ()> + Send,
    {
        self.options
            .notification_handler
            .get_or_insert_default()
            .subscribe(event, handler);
    }

    /// Unsubscribe a handler from the `event`
    pub fn unsubscribe(&mut self, event: impl AsRef<str>) {
        if let Some(notification_handler) = &self.options.notification_handler {
            notification_handler.unsubscribe(event);
        }
    }
}

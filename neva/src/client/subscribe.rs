﻿//! Additional helper methods for [`Client`] for variety notification subscription

use std::future::Future;
use super::Client;
use crate::types::notification::Notification;

impl Client {
    /// Maps a `handler` to the `notifications/resources/updated` event
    pub fn on_resource_changed<F, R>(&mut self, handler: F)
    where
        F: Fn(Notification) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = ()> + Send
    {
        assert!(
            self.is_resource_subscription_supported(), 
            "Server does not support resource subscriptions"
        );
        
        self.subscribe(
            crate::types::resource::commands::UPDATED,
            handler
        );
    }
    
    /// Maps a `handler` to the `notifications/resources/list_changed` event
    pub fn on_resources_changed<F, R>(&mut self, handler: F)
    where
        F: Fn(Notification) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = ()> + Send
    {
        assert!(
            self.is_resource_list_changed_supported(),
            "Server does not support resource list changed events"
        );
        
        self.subscribe(
            crate::types::resource::commands::LIST_CHANGED, 
            handler
        );
    }

    /// Maps a `handler` to the `notifications/tools/list_changed` event
    pub fn on_tools_changed<F, R>(&mut self, handler: F)
    where
        F: Fn(Notification) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = ()> + Send
    {
        assert!(
            self.is_tools_list_changed_supported(),
            "Server does not support tools list changed events"
        );
        
        self.subscribe(
            crate::types::tool::commands::LIST_CHANGED,
            handler
        );
    }

    /// Maps a `handler` to the `notifications/prompts/list_changed` event
    pub fn on_prompts_changed<F, R>(&mut self, handler: F)
    where
        F: Fn(Notification) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = ()> + Send
    {
        assert!(
            self.is_prompts_list_changed_supported(),
            "Server does not support prompts list changed events"
        );
        
        self.subscribe(
            crate::types::prompt::commands::LIST_CHANGED,
            handler
        );
    }
}
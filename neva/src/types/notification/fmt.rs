//! A generic tracing/logging formatting layer for notifications

use once_cell::sync::Lazy;
use dashmap::DashMap;
use tokio::sync::mpsc::{channel, Sender, UnboundedSender};
use tracing::{Event, Id, Subscriber, field::Visit};
use tracing::field::Field;
use tracing::span::Attributes;
use tracing_subscriber::{layer::Context, Layer};
use tracing_subscriber::registry::LookupSpan;
use std::io::{self, Write};
use crate::types::{
    notification::{Notification, formatter::build_notification},
    Message
};

const MCP_SESSION_ID: &str = "mcp_session_id";

static LOG_REGISTRY: Lazy<DashMap<String, UnboundedSender<Message>>> =
    Lazy::new(DashMap::new);

#[inline]
pub(crate) fn register_log_sender(key: String, sender: UnboundedSender<Message>) {
    LOG_REGISTRY.insert(key, sender);
}

#[inline]
pub(crate) fn unregister_log_sender(key: &str) {
    LOG_REGISTRY.remove(key);
}

#[inline]
pub(crate) fn send(message: Message) {
    let Some(session_id) = message.session_id() else { return; };
    if let Some(sender) = LOG_REGISTRY.get(session_id) {
        let _ = sender.send(message);
    }
}

/// Creates a custom tracing layer that delivers messages to MCP Client
/// 
/// # Example
/// ```no_run
/// use tracing_subscriber::prelude::*;
/// use neva::types::notification;
/// 
/// tracing_subscriber::registry()
///     .with(notification::fmt::layer())
///     .init();
/// ```
pub fn layer() -> MpscLayer {
    let (tx, mut rx) = channel::<Notification>(100);
    tokio::spawn(async move {
        while let Some(notification) = rx.recv().await {
            send(notification.into());
        }
    });
    MpscLayer {
        sender: NotificationSender::new(tx)
    }
}

/// Keeps a [`Sender`] 
struct NotificationSender {
    sender: Sender<Notification>,
}

impl NotificationSender {
    fn new(sender: Sender<Notification>) -> Self {
        Self { sender }
    }

    fn send_notification(&self, notification: Notification) {
        let _ = self.sender.try_send(notification);
    }
}

/// Represents a custom tracing layer that delivers messages to MCP Client
///
/// # Example
/// ```no_run
/// use tracing_subscriber::prelude::*;
/// use neva::types::notification;
///
/// tracing_subscriber::registry()
///     .with(notification::fmt::layer())
///     .init();
/// ```
pub struct MpscLayer {
    sender: NotificationSender
}

impl<S> Layer<S> for MpscLayer
where
    S: Subscriber  + for<'a> LookupSpan<'a>,
{
    #[inline]
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut visitor = SpanVisitor { session_id: None };
        attrs.record(&mut visitor);
        if let Some(span) = ctx.span(id) {
            if let Some(mcp_session_id) = visitor.session_id {
                span
                    .extensions_mut()
                    .insert(mcp_session_id);
            }
        }
    }
    
    #[inline]
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut notification = build_notification(event);
        if let Some(span) = ctx.event_span(event) {
            notification.session_id = span
                .extensions()
                .get::<String>()
                .cloned();
            self.sender.send_notification(notification);            
        } else {
            let mut stderr = io::stderr();
            let json = serde_json::to_string(&notification).unwrap();
            let _ = writeln!(stderr, "{}", json);
        }
    }
}

struct SpanVisitor {
    session_id: Option<String>,
}

impl Visit for SpanVisitor {
    #[inline]
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == MCP_SESSION_ID {
            self.session_id = Some(value.into());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // fallback if id was passed as %mcp_session_id or something else
        if field.name() == MCP_SESSION_ID && self.session_id.is_none() {
            self.session_id = Some(format!("{:?}", value));
        }
    }
}

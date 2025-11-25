//! A generic tracing/logging formatting layer for notifications

use std::io::{self, Write};
use once_cell::sync::Lazy;
use tokio::sync::mpsc::{channel, Sender};
use crate::shared::MessageRegistry;
use crate::types::notification::{
    Notification, 
    formatter::build_notification
};
use tracing::{
    {Event, Id, Subscriber, field::Visit},
    span::Attributes,
    field::Field
};
use tracing_subscriber::{
    {layer::Context, Layer},
    registry::LookupSpan
};

const MCP_SESSION_ID: &str = "mcp_session_id";

pub(crate) static LOG_REGISTRY: Lazy<MessageRegistry> =
    Lazy::new(MessageRegistry::new);

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
            let _ = LOG_REGISTRY.send(notification.into());
        }
    });
    MpscLayer {
        sender: NotificationSender::new(tx)
    }
}

/// Keeps a [`Sender`] 
#[derive(Debug)]
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
#[derive(Debug)]
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
        if let Some(span) = ctx.span(id)
            && let Some(mcp_session_id) = visitor.session_id {
            span
                .extensions_mut()
                .insert(mcp_session_id);
        }
    }
    
    #[inline]
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut notification = build_notification(event);
        if let Some(span) = ctx.event_span(event) {
            notification.session_id = span
                .extensions()
                .get::<uuid::Uuid>()
                .cloned();
            self.sender.send_notification(notification);            
        } else {
            let mut stderr = io::stderr();
            let json = serde_json::to_string(&notification).unwrap();
            let _ = writeln!(stderr, "{json}");
        }
    }
}

struct SpanVisitor {
    session_id: Option<uuid::Uuid>,
}

impl Visit for SpanVisitor {
    #[inline]
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == MCP_SESSION_ID
            && let Ok(session_id) = uuid::Uuid::parse_str(value) {
            self.session_id = Some(session_id);
        }
    }
    
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // fallback if id was passed as %mcp_session_id or something else
        if field.name() == MCP_SESSION_ID && self.session_id.is_none() {
            let formatted = format!("{value:?}");
            let stripped = formatted
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(&formatted);
            
            if let Ok(session_id) = uuid::Uuid::parse_str(stripped) { 
                self.session_id = Some(session_id);
            } 
        }
    }
}

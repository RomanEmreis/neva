//! A generic tracing/logging formatting layer for notifications

use crate::shared::MessageRegistry;
#[cfg(not(feature = "legacy-spec"))]
use crate::types::Message;
#[cfg(not(feature = "legacy-spec"))]
use crate::types::notification::LoggingLevel;
use crate::types::notification::formatter::build_notification;
use once_cell::sync::Lazy;
use std::io::{self, Write};
use tracing::{
    field::Field,
    span::Attributes,
    {Event, Id, Subscriber, field::Visit},
};
use tracing_subscriber::{
    registry::LookupSpan,
    {Layer, layer::Context},
};

const MCP_SESSION_ID: &str = "mcp_session_id";

/// Span field carrying the request-scoped minimum severity (2026-07-28), as the
/// [`LoggingLevel`] RFC-5424 severity rank rather than a redundant string.
#[cfg(not(feature = "legacy-spec"))]
pub(super) const MCP_LOG_LEVEL: &str = "mcp_log_level";

pub(crate) static LOG_REGISTRY: Lazy<MessageRegistry> = Lazy::new(MessageRegistry::new);

/// Creates a custom tracing layer that delivers messages to MCP Client
///
/// This layer routes notifications to a connected client. On the legacy HTTP
/// transport that is the session-scoped SSE `GET` stream.
///
/// # MCP 2026-07-28
///
/// The 2026-07-28 HTTP transport is stateless (no `GET`/SSE stream, no sessions), so
/// request-scoped notifications flow on the *originating request's `POST`
/// response stream*, per the spec. This layer routes each event to the
/// per-request sink registered by the POST handler (keyed by the per-`POST`
/// session id the request span carries); the `POST` reply is then a
/// `text/event-stream` carrying the notifications followed by the response.
/// The same works over stdio, where notifications interleave on stdout.
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
    MpscLayer
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
#[derive(Debug, Default)]
pub struct MpscLayer;

impl<S> Layer<S> for MpscLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    #[inline]
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        record_span_context(attrs, id, &ctx);
    }

    #[inline]
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let notification = build_notification(event);
        if let Some(span) = ctx.event_span(event) {
            let mut notification = notification;
            // Resolve the request context through the whole span scope, not just
            // the immediate span: an event emitted from a child span (e.g. a
            // `#[tracing::instrument]` handler) sits below the `request` span
            // that carries the extensions.
            notification.session_id = span
                .scope()
                .find_map(|s| s.extensions().get::<uuid::Uuid>().cloned());

            // 2026-07-28: `logging/setLevel` is gone; the level is request-scoped. Deliver
            // a `notifications/message` only when the originating request carried
            // `io.modelcontextprotocol/logLevel` and this event is at or above
            // that severity. Without a requested level, log messages are
            // suppressed. Progress notifications are never gated this way.
            #[cfg(not(feature = "legacy-spec"))]
            if notification.method.as_str() == crate::types::notification::commands::MESSAGE {
                let requested = span.scope().find_map(|s| {
                    s.extensions()
                        .get::<super::formatter::MinLogSeverity>()
                        .map(|m| m.0)
                });
                // Filter on the notification's own level (which preserves MCP
                // severities), not the lossy tracing level of the event.
                let event_severity = super::formatter::notification_severity(&notification)
                    .unwrap_or_else(|| LoggingLevel::from(event.metadata().level()).severity());
                if !super::formatter::message_delivered(requested, event_severity) {
                    return;
                }
            }

            // 2026-07-28: request-scoped notifications flow on the originating request's
            // `POST` response stream (there is no session SSE `GET`). Route to the
            // per-request sink registered by `handle_post` for this `POST` session
            // id; fall through to the legacy session-SSE path otherwise.
            #[cfg(not(feature = "legacy-spec"))]
            if let Some(session_id) = notification.session_id
                && let Some(sink) = super::sink::REQUEST_NOTIFICATIONS.get(&session_id)
            {
                let _ = sink.try_send(Message::Notification(notification));

                return;
            }

            // Legacy: the session-scoped SSE `GET` stream. Queued here and now,
            // synchronously, so a notification a handler emits is on the
            // stream's channel before the handler returns -- routing it through
            // a channel of its own once cost a scheduler hop, and a handler that
            // never awaits (it reports progress and returns) would not yield
            // until it was done, letting its own response overtake the progress
            // it had already reported.
            let _ = LOG_REGISTRY.send(notification.into());
        } else {
            let mut stderr = io::stderr();
            let json = serde_json::to_string(&notification).unwrap();
            let _ = writeln!(stderr, "{json}");
        }
    }
}

/// A tracing [`Layer`] that only records the MCP span context (session id and,
/// under MCP 2026-07-28, the request-scoped `LoggingLevel`) into a span's
/// extensions.
///
/// This is an optimization, not a requirement. The [`layer`] function's
/// `MpscLayer` records the context itself, and the stdio
/// [`NotificationFormatter`](super::NotificationFormatter) resolves the
/// request-scoped level from the span fields `fmt::Layer` already records when no
/// extension is present -- so both emission paths apply the request-scoped
/// `notifications/message` filter with or without this layer. Adding it next to
/// the stdio formatter just replaces that lookup with a typed one.
#[derive(Debug, Default)]
pub struct SpanContextLayer;

impl<S> Layer<S> for SpanContextLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    #[inline]
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        record_span_context(attrs, id, &ctx);
    }
}

/// Creates a [`SpanContextLayer`].
///
/// # Example
/// ```no_run
/// use tracing_subscriber::prelude::*;
/// use neva::types::notification;
///
/// tracing_subscriber::registry()
///     .with(notification::fmt::span_context())
///     .with(tracing_subscriber::fmt::layer().event_format(notification::NotificationFormatter))
///     .init();
/// ```
pub fn span_context() -> SpanContextLayer {
    SpanContextLayer
}

/// Records the MCP `request` span fields (`mcp_session_id`, and under MCP 2026-07-28
/// `mcp_log_level`) into the span's typed extensions. Shared by [`MpscLayer`]
/// and [`SpanContextLayer`] so both emission paths read an identical context.
#[inline]
fn record_span_context<S>(attrs: &Attributes<'_>, id: &Id, ctx: &Context<'_, S>)
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    let mut visitor = SpanVisitor::default();
    attrs.record(&mut visitor);
    if let Some(span) = ctx.span(id) {
        if let Some(mcp_session_id) = visitor.session_id {
            span.extensions_mut().insert(mcp_session_id);
        }
        // 2026-07-28: remember the request-scoped minimum severity so the emission path
        // can filter `notifications/message` for events fired within this request.
        #[cfg(not(feature = "legacy-spec"))]
        if let Some(min) = visitor.min_severity {
            span.extensions_mut()
                .insert(super::formatter::MinLogSeverity(min));
        }
    }
}

#[derive(Default)]
struct SpanVisitor {
    session_id: Option<uuid::Uuid>,
    /// Minimum severity requested for this request (`mcp_log_level` span field),
    /// carried as the level's RFC-5424 severity rank.
    #[cfg(not(feature = "legacy-spec"))]
    min_severity: Option<u8>,
}

impl Visit for SpanVisitor {
    #[inline]
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == MCP_SESSION_ID
            && let Ok(session_id) = uuid::Uuid::parse_str(value)
        {
            self.session_id = Some(session_id);
        }
    }

    #[cfg(not(feature = "legacy-spec"))]
    #[inline]
    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == MCP_LOG_LEVEL {
            self.min_severity = Some(value as u8);
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

// The legacy emission path: a session-scoped SSE `GET` stream fed through
// `LOG_REGISTRY`, with no per-request sink to route to.
#[cfg(all(test, feature = "legacy-spec"))]
mod legacy_tests {
    use tracing_subscriber::prelude::*;

    /// Emits inside a `request` span carrying `session_id` and returns whatever
    /// the session's channel holds by the time the emitting code is done --
    /// nothing is awaited in between on purpose.
    fn emit_and_drain(session_id: uuid::Uuid, emit: impl FnOnce()) -> Vec<serde_json::Value> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        super::LOG_REGISTRY.register(session_id, 1, tx);

        let subscriber = tracing_subscriber::registry().with(super::MpscLayer);
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("request", mcp_session_id = session_id.to_string());
            let _entered = span.enter();
            emit();
        });

        super::LOG_REGISTRY.unregister(&session_id);

        let mut got = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            got.push(serde_json::to_value(&msg).unwrap());
        }
        got
    }

    #[test]
    fn a_progress_report_is_on_the_stream_before_the_emitter_returns() {
        // The reports and the call's result travel on two different connections
        // here, so nothing orders them but the emitting task yielding. Queueing
        // synchronously is what keeps a report that was made *during* the call
        // from arriving after the call's own result -- by which time the client
        // has stopped looking.
        let got = emit_and_drain(uuid::Uuid::new_v4(), || {
            for value in [0, 50, 100] {
                tracing::info!(target: "progress", token = "tok-1", value = value, total = 100);
            }
        });

        let progress = got
            .iter()
            .filter(|m| m["method"] == "notifications/progress")
            .collect::<Vec<_>>();
        assert_eq!(progress.len(), 3, "got: {got:?}");
        assert_eq!(progress[0]["params"]["progressToken"], "tok-1");
        assert_eq!(progress[0]["params"]["progress"], 0.0);
        assert_eq!(progress[2]["params"]["progress"], 100.0);
        assert_eq!(progress[2]["params"]["total"], 100.0);
    }

    #[test]
    fn a_log_message_travels_the_same_way() {
        let got = emit_and_drain(uuid::Uuid::new_v4(), || {
            tracing::warn!(logger = "tool", "something happened");
        });

        assert_eq!(got.len(), 1, "got: {got:?}");
        assert_eq!(got[0]["method"], "notifications/message");
        assert_eq!(got[0]["params"]["level"], "warning");
        assert_eq!(got[0]["params"]["data"]["message"], "something happened");
    }

    #[test]
    fn an_event_for_an_unknown_session_is_dropped_rather_than_kept() {
        // No stream to put it on. The registry says so, and nothing queues up
        // waiting for a session that may never open one.
        let subscriber = tracing_subscriber::registry().with(super::MpscLayer);
        tracing::subscriber::with_default(subscriber, || {
            let span =
                tracing::info_span!("request", mcp_session_id = uuid::Uuid::new_v4().to_string());
            let _entered = span.enter();
            tracing::warn!(logger = "tool", "nobody is listening");
        });
    }
}

// End-to-end coverage of 2026-07-28 request-scoped logging through the real tracing
// pipeline: the `span_context` layer records `mcp_log_level` into a span, and
// `NotificationFormatter` filters `notifications/message` accordingly.
#[cfg(all(test, not(feature = "legacy-spec")))]
mod tests {
    use crate::types::notification::{LoggingLevel, NotificationFormatter};
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;
    use tracing_subscriber::prelude::*;

    #[derive(Clone)]
    struct BufWriter(Arc<Mutex<Vec<u8>>>);

    struct BufGuard(Arc<Mutex<Vec<u8>>>);

    impl Write for BufGuard {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for BufWriter {
        type Writer = BufGuard;
        fn make_writer(&'a self) -> Self::Writer {
            BufGuard(self.0.clone())
        }
    }

    /// Drives the emit path within a `request` span carrying `log_level` (when
    /// `Some`) and returns the JSON lines the stdio formatter would send.
    fn emit_within_request(log_level: Option<LoggingLevel>) -> Vec<String> {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry()
            .with(super::span_context())
            .with(
                tracing_subscriber::fmt::layer()
                    .event_format(NotificationFormatter)
                    .with_writer(BufWriter(buf.clone())),
            );

        tracing::subscriber::with_default(subscriber, || {
            let span = match log_level {
                Some(level) => {
                    tracing::info_span!("request", mcp_log_level = u64::from(level.severity()))
                }
                None => tracing::info_span!("request"),
            };
            let _entered = span.enter();
            tracing::error!(logger = "tool", "error message");
            tracing::warn!(logger = "tool", "warning message");
            tracing::info!(logger = "tool", "info message");
            tracing::debug!(logger = "tool", "debug message");
        });

        let raw = buf.lock().unwrap().clone();
        String::from_utf8(raw)
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn levels(lines: &[String]) -> Vec<String> {
        lines
            .iter()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|v| v["method"] == "notifications/message")
            .filter_map(|v| v["params"]["level"].as_str().map(str::to_owned))
            .collect()
    }

    #[test]
    fn delivers_messages_at_or_above_requested_level() {
        // Requesting `warning` delivers error + warning, drops info + debug.
        let lines = emit_within_request(Some(LoggingLevel::Warning));
        let got = levels(&lines);
        assert!(got.contains(&"error".to_owned()), "got: {got:?}");
        assert!(got.contains(&"warning".to_owned()), "got: {got:?}");
        assert!(!got.contains(&"info".to_owned()), "got: {got:?}");
        assert!(!got.contains(&"debug".to_owned()), "got: {got:?}");
    }

    #[test]
    fn delivers_everything_at_debug() {
        let got = levels(&emit_within_request(Some(LoggingLevel::Debug)));
        for lvl in ["error", "warning", "info", "debug"] {
            assert!(got.contains(&lvl.to_owned()), "missing {lvl}, got: {got:?}");
        }
    }

    #[test]
    fn suppresses_all_messages_without_requested_level() {
        // No `logLevel` on the request => the server emits no log notifications.
        let got = levels(&emit_within_request(None));
        assert!(got.is_empty(), "expected none, got: {got:?}");
    }

    #[test]
    fn preserves_mcp_specific_severity_past_tracing() {
        use crate::types::notification::LogMessage;

        // `LogMessage::write()` downgrades to a tracing ERROR for both, but the
        // MCP level is preserved on the wire and used for filtering: a client
        // requesting `emergency` gets the emergency log and not the error one.
        let buf = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry()
            .with(super::span_context())
            .with(
                tracing_subscriber::fmt::layer()
                    .event_format(NotificationFormatter)
                    .with_writer(BufWriter(buf.clone())),
            );

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!(
                "request",
                mcp_log_level = u64::from(LoggingLevel::Emergency.severity())
            );
            let _entered = span.enter();
            LogMessage::new(LoggingLevel::Emergency, None, None).write();
            LogMessage::new(LoggingLevel::Error, None, None).write();
        });

        let raw = buf.lock().unwrap().clone();
        let got: Vec<String> = String::from_utf8(raw)
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_owned)
            .collect();
        let got = levels(&got);
        assert!(got.contains(&"emergency".to_owned()), "got: {got:?}");
        assert!(!got.contains(&"error".to_owned()), "got: {got:?}");
    }

    /// Drives `MpscLayer` (the HTTP/SSE emit path) with the request context on an
    /// *ancestor* span, the way a `#[tracing::instrument]` handler emits: from a
    /// child span that carries no MCP fields of its own.
    #[tokio::test]
    async fn routes_events_from_nested_spans_to_the_request_sink() {
        let session_id = uuid::Uuid::new_v4();
        let mut sink_rx = super::super::sink::register(session_id, 8, false).await;

        // The legacy session-SSE registry, standing by under the same id: if the
        // 2026-07-28 path ever fell through, this is where the event would land.
        let (fallback_tx, mut fallback_rx) = tokio::sync::mpsc::channel(8);
        super::LOG_REGISTRY.register(session_id, 1, fallback_tx);

        let subscriber = tracing_subscriber::registry().with(super::MpscLayer);

        tracing::subscriber::with_default(subscriber, || {
            let request = tracing::info_span!(
                "request",
                mcp_session_id = session_id.to_string(),
                mcp_log_level = u64::from(LoggingLevel::Debug.severity())
            );
            let _entered = request.enter();
            let handler = tracing::info_span!("handler");
            let _handler = handler.enter();
            tracing::warn!(logger = "tool", "nested message");
        });

        super::super::sink::unregister(&session_id);
        super::LOG_REGISTRY.unregister(&session_id);

        let msg = sink_rx
            .try_recv()
            .expect("an event from a nested span must still reach the request sink");
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["method"], "notifications/message");
        assert_eq!(json["params"]["data"]["message"], "nested message");
        // ...and it must not fall through to the legacy session-SSE registry,
        // which has no reader on the 2026-07-28 stateless transport.
        assert!(
            fallback_rx.try_recv().is_err(),
            "request-scoped notification leaked to the legacy path"
        );
    }
}

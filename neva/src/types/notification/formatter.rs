//! A tracing/logging formatter for notifications

use std::collections::BTreeMap;
use tracing::level_filters::LevelFilter;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::{
    field::Visit,
    fmt::{FmtContext, FormatEvent, format::FormatFields, format::Writer},
    registry::LookupSpan,
};

use crate::types::ProgressToken;
use crate::types::notification::{LogMessage, LoggingLevel, Notification};

/// A formatter that formats tracing events into MCP notification logs
///
/// This is the stdio emission path: each event is written to stdout as a
/// JSON-RPC `notifications/message` (or `notifications/progress`), interleaved
/// with the transport's other traffic.
///
/// # MCP 2026-07-28
///
/// `logging/setLevel` is gone and the level is request-scoped, so a
/// `notifications/message` is emitted only when the originating request carried
/// `io.modelcontextprotocol/logLevel` and the event is at or above that
/// severity. The level is picked up from the request span automatically -- the
/// setup below needs nothing else. Adding
/// [`notification::fmt::span_context()`](super::fmt::span_context) alongside
/// resolves it from a typed span extension instead, which is marginally cheaper.
///
/// # Examples
/// ```no_run
/// use tracing_subscriber::prelude::*;
/// use neva::types::notification;
///
/// tracing_subscriber::registry()
///     .with(tracing_subscriber::fmt::layer().event_format(notification::NotificationFormatter))
///     .init();
/// ```
#[allow(missing_debug_implementations)]
pub struct NotificationFormatter;

impl From<&Level> for LoggingLevel {
    #[inline]
    fn from(level: &Level) -> Self {
        match *level {
            Level::ERROR => LoggingLevel::Error,
            Level::WARN => LoggingLevel::Warning,
            Level::INFO => LoggingLevel::Info,
            Level::DEBUG => LoggingLevel::Debug,
            Level::TRACE => LoggingLevel::Debug,
        }
    }
}

impl From<LevelFilter> for LoggingLevel {
    #[inline]
    fn from(level: LevelFilter) -> Self {
        match level {
            LevelFilter::ERROR => LoggingLevel::Error,
            LevelFilter::WARN => LoggingLevel::Warning,
            LevelFilter::INFO => LoggingLevel::Info,
            LevelFilter::DEBUG => LoggingLevel::Debug,
            LevelFilter::TRACE => LoggingLevel::Debug,
            _ => LoggingLevel::Info,
        }
    }
}

impl From<LoggingLevel> for LevelFilter {
    #[inline]
    fn from(level: LoggingLevel) -> Self {
        match level {
            LoggingLevel::Alert => LevelFilter::ERROR,
            LoggingLevel::Critical => LevelFilter::ERROR,
            LoggingLevel::Emergency => LevelFilter::ERROR,
            LoggingLevel::Error => LevelFilter::ERROR,
            LoggingLevel::Warning => LevelFilter::WARN,
            LoggingLevel::Notice => LevelFilter::WARN,
            LoggingLevel::Info => LevelFilter::INFO,
            LoggingLevel::Debug => LevelFilter::DEBUG,
        }
    }
}

impl From<LoggingLevel> for Level {
    #[inline]
    fn from(level: LoggingLevel) -> Self {
        match level {
            LoggingLevel::Alert => Level::ERROR,
            LoggingLevel::Critical => Level::ERROR,
            LoggingLevel::Emergency => Level::ERROR,
            LoggingLevel::Error => Level::ERROR,
            LoggingLevel::Warning => Level::WARN,
            LoggingLevel::Notice => Level::WARN,
            LoggingLevel::Info => Level::INFO,
            LoggingLevel::Debug => Level::DEBUG,
        }
    }
}

impl<S, N> FormatEvent<S, N> for NotificationFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result {
        let notification = build_notification(event);

        // 2026-07-28: the stdio emission path honors the same request-scoped level as
        // the HTTP path. A `notifications/message` is written only when the
        // originating request carried `io.modelcontextprotocol/logLevel` and
        // this event is at or above that severity. Progress notifications are
        // not gated.
        #[cfg(not(feature = "legacy-spec"))]
        if notification.method.as_str() == crate::types::notification::commands::MESSAGE {
            let requested = requested_severity(_ctx);

            // Filter on the notification's own level (which preserves MCP
            // severities), not the lossy tracing level of the event.
            let event_severity = notification_severity(&notification)
                .unwrap_or_else(|| LoggingLevel::from(event.metadata().level()).severity());
            if !message_delivered(requested, event_severity) {
                return Ok(());
            }
        }

        let json = serde_json::to_string(&notification).unwrap();
        writeln!(writer, "{json}")
    }
}

/// The request-scoped minimum severity recorded on a `request` span's
/// extensions (MCP 2026-07-28), as an RFC-5424 severity rank. Both emission
/// paths ([`NotificationFormatter`] for stdio and [`super::fmt`] for HTTP) read
/// it back to filter `notifications/message`.
#[cfg(not(feature = "legacy-spec"))]
#[derive(Debug, Clone, Copy)]
pub(super) struct MinLogSeverity(pub(super) u8);

/// The minimum severity the originating request asked for, resolved from the
/// event's span scope (the `request` span may be an ancestor, e.g. when a
/// `#[tracing::instrument]` handler emits from its own span).
///
/// Prefers the typed [`MinLogSeverity`] extension recorded by
/// [`span_context`](super::fmt::span_context), and otherwise recovers the rank
/// from the span fields `fmt::Layer` records for every span on its own. That
/// fallback is what lets a formatter-only subscriber -- `fmt::layer()
/// .event_format(NotificationFormatter)` with no other layer -- honor
/// `io.modelcontextprotocol/logLevel` without extra configuration.
#[cfg(not(feature = "legacy-spec"))]
fn requested_severity<S, N>(ctx: &FmtContext<'_, S, N>) -> Option<u8>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    ctx.event_scope()?.find_map(|span| {
        let ext = span.extensions();
        if let Some(min) = ext.get::<MinLogSeverity>() {
            return Some(min.0);
        }
        ext.get::<tracing_subscriber::fmt::FormattedFields<N>>()
            .and_then(|fields| severity_from_fields(&fields.fields))
    })
}

/// Reads the `mcp_log_level` rank out of a span's formatted fields, accepting
/// both the default `key=value` shape and the JSON one (`"key":value`).
#[cfg(not(feature = "legacy-spec"))]
fn severity_from_fields(fields: &str) -> Option<u8> {
    let fields = strip_ansi(fields);
    let rest = fields.split_once(super::fmt::MCP_LOG_LEVEL)?.1;
    rest.trim_start_matches(['"', ':', '=', ' '])
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

/// Removes ANSI escape sequences from formatted span fields.
///
/// `fmt::Layer` styles field names by default, so the rank sits behind escape
/// sequences -- and those contain digits of their own, which would otherwise be
/// read as the level. Borrows when there is nothing to strip.
#[cfg(not(feature = "legacy-spec"))]
fn strip_ansi(fields: &str) -> std::borrow::Cow<'_, str> {
    const ESC: char = '\u{1b}';
    if !fields.contains(ESC) {
        return std::borrow::Cow::Borrowed(fields);
    }

    let mut out = String::with_capacity(fields.len());
    let mut chars = fields.chars();
    while let Some(c) = chars.next() {
        if c != ESC {
            out.push(c);
            continue;
        }
        // CSI: `ESC [ <params> <final>`, where the final byte is 0x40..=0x7E.
        if chars.next() != Some('[') {
            continue;
        }
        for c in chars.by_ref() {
            if ('\u{40}'..='\u{7e}').contains(&c) {
                break;
            }
        }
    }
    std::borrow::Cow::Owned(out)
}

/// Whether a `notifications/message` at `event_severity` should be delivered to
/// a client that requested a minimum severity (MCP 2026-07-28, request-scoped
/// logging). No requested level means no delivery. Both values are
/// [`LoggingLevel::severity`] ranks.
#[cfg(not(feature = "legacy-spec"))]
#[inline]
pub(super) fn message_delivered(requested: Option<u8>, event_severity: u8) -> bool {
    requested.map(|min| event_severity >= min).unwrap_or(false)
}

/// Reads the RFC-5424 severity rank of a `notifications/message`'s own level,
/// so filtering matches the level actually delivered to the client (which, via
/// [`build_notification`], preserves MCP-specific severities).
#[cfg(not(feature = "legacy-spec"))]
#[inline]
pub(super) fn notification_severity(notification: &Notification) -> Option<u8> {
    notification
        .params
        .as_ref()
        .and_then(|params| params.get("level"))
        .and_then(|level| serde_json::from_value::<LoggingLevel>(level.clone()).ok())
        .map(LoggingLevel::severity)
}

#[inline]
pub(super) fn build_notification(event: &Event<'_>) -> Notification {
    let meta = event.metadata();
    let level = meta.level();
    let fields = extract_fields(event);

    match meta.target() {
        "progress" => {
            let token = fields
                .get("token")
                .map(|v| serde_json::from_value::<ProgressToken>(v.clone()).unwrap());

            let total = fields
                .get("total")
                .map(|v| v.to_string().replace("\"", "").parse().unwrap());

            let value = fields
                .get("value")
                .map(|v| v.to_string().replace("\"", "").parse().unwrap());

            token.unwrap().notify(value.unwrap(), total).into()
        }
        _ => {
            let logger = fields
                .get("logger")
                .map(|v| v.to_string().replace("\"", ""));

            // Remove `logger` from data map
            let mut data_map = fields.clone();
            data_map.remove("logger");

            // An explicit MCP level (from `LogMessage::write`) preserves
            // severities tracing cannot express; otherwise map the tracing level.
            let level = data_map
                .remove("mcp_level")
                .and_then(|v| serde_json::from_value::<LoggingLevel>(v).ok())
                .unwrap_or_else(|| level.into());

            let log = LogMessage {
                level,
                data: serde_json::to_value(data_map).ok(),
                logger,
            };

            Notification::from(log)
        }
    }
}

#[inline]
fn extract_fields<'a>(event: &Event<'a>) -> BTreeMap<&'a str, serde_json::Value> {
    let mut visitor = Visitor {
        map: BTreeMap::new(),
    };
    event.record(&mut visitor);
    visitor.map
}

struct Visitor<'a> {
    map: BTreeMap<&'a str, serde_json::Value>,
}

impl Visit for Visitor<'_> {
    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.map.insert(field.name(), value.into());
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.map.insert(field.name(), value.into());
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.map.insert(field.name(), value.into());
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.map.insert(field.name(), value.into());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        // Only use this if nothing else handled it
        if !self.map.contains_key(field.name()) {
            let formatted = format!("{value:?}");
            let value =
                serde_json::to_value(&formatted).unwrap_or(serde_json::Value::String(formatted));
            self.map.insert(field.name(), value);
        }
    }
}

#[cfg(all(test, not(feature = "legacy-spec")))]
mod tests {
    use super::{message_delivered, severity_from_fields};
    use crate::types::notification::LoggingLevel;

    fn sev(level: LoggingLevel) -> u8 {
        level.severity()
    }

    #[test]
    fn no_requested_level_delivers_nothing() {
        for level in [
            LoggingLevel::Debug,
            LoggingLevel::Error,
            LoggingLevel::Emergency,
        ] {
            assert!(!message_delivered(None, sev(level)));
        }
    }

    #[test]
    fn delivers_at_or_above_requested_severity() {
        // Requested `warning` delivers warning and everything more severe.
        let min = sev(LoggingLevel::Warning);
        assert!(message_delivered(Some(min), sev(LoggingLevel::Warning)));
        assert!(message_delivered(Some(min), sev(LoggingLevel::Error)));
        assert!(message_delivered(Some(min), sev(LoggingLevel::Emergency)));
    }

    #[test]
    fn drops_below_requested_severity() {
        let warn = sev(LoggingLevel::Warning);
        assert!(!message_delivered(Some(warn), sev(LoggingLevel::Info)));
        assert!(!message_delivered(Some(warn), sev(LoggingLevel::Debug)));
        assert!(!message_delivered(
            Some(sev(LoggingLevel::Error)),
            sev(LoggingLevel::Notice)
        ));
    }

    #[test]
    fn reads_the_requested_rank_from_formatted_span_fields() {
        // Default field formatter, and with the session id alongside.
        assert_eq!(severity_from_fields("mcp_log_level=3"), Some(3));
        assert_eq!(
            severity_from_fields("mcp_session_id=\"3f1a\" mcp_log_level=7"),
            Some(7)
        );
        // JSON field formatter.
        assert_eq!(
            severity_from_fields(r#"{"mcp_session_id":"3f1a","mcp_log_level":0}"#),
            Some(0)
        );
        // ANSI-styled field names (the `fmt::Layer` default): the escape
        // sequences carry digits of their own, which must not be read as a rank.
        assert_eq!(
            severity_from_fields("\u{1b}[3mmcp_log_level\u{1b}[0m\u{1b}[2m=\u{1b}[0m3"),
            Some(3)
        );
        assert_eq!(
            severity_from_fields("\u{1b}[3mmcp_log_level\u{1b}[0m\u{1b}[2m=\u{1b}[0m0"),
            Some(0)
        );
        // Spans that carry no requested level.
        assert_eq!(severity_from_fields(""), None);
        assert_eq!(severity_from_fields("mcp_session_id=\"3f1a\""), None);
        assert_eq!(
            severity_from_fields("\u{1b}[3mmcp_session_id\u{1b}[0m=\u{1b}[0m\"3f1a\""),
            None
        );
    }

    /// A formatter-only subscriber -- no `span_context()` layer -- must still
    /// honor `io.modelcontextprotocol/logLevel`: `fmt::Layer` records the span
    /// fields itself, so the requested level is recoverable without any extra
    /// configuration.
    #[test]
    fn honors_the_requested_level_without_a_span_context_layer() {
        use crate::types::notification::NotificationFormatter;
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

        let buf = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .event_format(NotificationFormatter)
                .with_writer(BufWriter(buf.clone())),
        );

        tracing::subscriber::with_default(subscriber, || {
            let request = tracing::info_span!(
                "request",
                mcp_log_level = u64::from(LoggingLevel::Warning.severity())
            );
            let _entered = request.enter();
            // Also from a nested span, the way an instrumented handler emits.
            let handler = tracing::info_span!("handler");
            let _handler = handler.enter();
            tracing::error!(logger = "tool", "delivered");
            tracing::info!(logger = "tool", "dropped");
        });

        let out = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(
            out.contains("delivered"),
            "requesting `warning` must deliver an error event, got: {out}"
        );
        assert!(
            !out.contains("dropped"),
            "an event below the requested level must be suppressed, got: {out}"
        );
    }

    /// Without a requested level nothing is emitted, layer or no layer -- the
    /// spec's suppression rule must not be defeated by the fallback.
    #[test]
    fn suppresses_everything_when_the_request_did_not_opt_in() {
        use crate::types::notification::NotificationFormatter;
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

        let buf = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .event_format(NotificationFormatter)
                .with_writer(BufWriter(buf.clone())),
        );

        tracing::subscriber::with_default(subscriber, || {
            let request = tracing::info_span!("request", mcp_session_id = "3f1a");
            let _entered = request.enter();
            tracing::error!(logger = "tool", "must not appear");
        });

        let out = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(out.is_empty(), "expected no output, got: {out}");
    }
}

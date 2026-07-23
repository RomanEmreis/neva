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

        // RC: the stdio emission path honors the same request-scoped level as
        // the HTTP path. A `notifications/message` is written only when the
        // originating request carried `io.modelcontextprotocol/logLevel` and
        // this event is at or above that severity (span context recorded by
        // [`super::fmt::span_context`]). Progress notifications are not gated.
        #[cfg(feature = "proto-2026-07-28-rc")]
        if notification.method.as_str() == crate::types::notification::commands::MESSAGE {
            let requested = _ctx.event_scope().and_then(|scope| {
                scope
                    .into_iter()
                    .find_map(|s| s.extensions().get::<MinLogSeverity>().map(|m| m.0))
            });

            let event_severity = LoggingLevel::from(event.metadata().level()).severity();
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
#[cfg(feature = "proto-2026-07-28-rc")]
#[derive(Debug, Clone, Copy)]
pub(super) struct MinLogSeverity(pub(super) u8);

/// Whether a `notifications/message` at `event_severity` should be delivered to
/// a client that requested a minimum severity (MCP 2026-07-28, request-scoped
/// logging). No requested level means no delivery. Both values are
/// [`LoggingLevel::severity`] ranks.
#[cfg(feature = "proto-2026-07-28-rc")]
#[inline]
pub(super) fn message_delivered(requested: Option<u8>, event_severity: u8) -> bool {
    requested.map(|min| event_severity >= min).unwrap_or(false)
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

            let log = LogMessage {
                level: level.into(),
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

#[cfg(all(test, feature = "proto-2026-07-28-rc"))]
mod tests {
    use super::message_delivered;
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
}

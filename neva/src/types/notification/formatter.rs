//! A tracing/logging formatter for notifications

use std::collections::BTreeMap;
use tracing::{Event, Subscriber, Level};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{
    fmt::{
        format::FormatFields, 
        FormatEvent, 
        FmtContext, 
        format::Writer
    },
    field::Visit,
    registry::LookupSpan,
};

use crate::types::notification::{
    LogMessage, 
    LoggingLevel, 
    Notification
};
use crate::types::ProgressToken;

/// A formatter that formats tracing events into MCP notification logs
pub struct NotificationFormatter;

impl From<&Level> for LoggingLevel {
    #[inline]
    fn from(level: &Level) -> Self {
        match *level { 
            Level::ERROR => LoggingLevel::Error,
            Level::WARN => LoggingLevel::Warning,
            Level::INFO => LoggingLevel::Info,
            Level::DEBUG => LoggingLevel::Debug,
            Level::TRACE => LoggingLevel::Debug
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
            _ => LoggingLevel::Info
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
        let json = serde_json::to_string(&notification).unwrap();
        writeln!(writer, "{}", json)
    }
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

            token.unwrap()
                .notify(value.unwrap(), total)
                .into()
        },
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
            self.map.insert(field.name(), serde_json::Value::String(format!("{:?}", value)));
        }
    }
}
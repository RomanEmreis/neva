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
        // 2026-07-28: the stdio emission path honors the same request-scoped level as
        // the HTTP path. A `notifications/message` is written only when the
        // originating request carried `io.modelcontextprotocol/logLevel` and
        // this event is at or above that severity. Progress notifications are
        // not gated. Decided before anything is built: see [`delivered`].
        #[cfg(not(feature = "legacy-spec"))]
        if !delivered(event, || requested_severity(_ctx)) {
            return Ok(());
        }

        let Some(notification) = build_notification(event) else {
            return Ok(());
        };
        let Ok(json) = serde_json::to_string(&notification) else {
            return Ok(());
        };

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

/// The target of an event that reports progress rather than logging.
const PROGRESS_TARGET: &str = "progress";

/// The field carrying an event's MCP level, which tracing's own levels cannot
/// all express: see `LogMessage::write`.
const MCP_LEVEL: &str = "mcp_level";

/// Whether an event becomes a notification the client is sent, under MCP
/// 2026-07-28's request-scoped logging.
///
/// Read off the event itself and the level the request asked for, so it is
/// settled before the notification is built. That ordering is the point: a
/// request that asked for no level is sent no log message at all, and every
/// event inside it used to be turned into JSON first and dropped after.
/// `requested` is asked only for an event that is a log message.
///
/// The level compared is the event's MCP level where it states one -- the
/// level the client would be sent -- and its tracing level otherwise.
#[cfg(not(feature = "legacy-spec"))]
pub(super) fn delivered(event: &Event<'_>, requested: impl FnOnce() -> Option<u8>) -> bool {
    if event.metadata().target() == PROGRESS_TARGET {
        return true;
    }
    let Some(min) = requested() else {
        return false;
    };

    message_delivered(Some(min), event_severity(event))
}

/// The RFC-5424 severity rank of the log message an event becomes: its
/// `mcp_level` field where that names a level, its tracing level otherwise --
/// the same choice [`build_notification`] makes.
#[cfg(not(feature = "legacy-spec"))]
fn event_severity(event: &Event<'_>) -> u8 {
    struct McpLevel(Option<LoggingLevel>);

    impl Visit for McpLevel {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            if field.name() == MCP_LEVEL {
                use serde::Deserialize as _;
                use serde::de::value::{Error, StrDeserializer};

                self.0 = LoggingLevel::deserialize(StrDeserializer::<Error>::new(value)).ok();
            }
        }

        fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
    }

    let mut level = McpLevel(None);
    event.record(&mut level);
    level
        .0
        .unwrap_or_else(|| LoggingLevel::from(event.metadata().level()))
        .severity()
}

/// The notification an event describes, if it describes one.
///
/// An event on the `progress` target is a progress notification; any other
/// is a log message.
#[inline]
pub(super) fn build_notification(event: &Event<'_>) -> Option<Notification> {
    let meta = event.metadata();
    let mut fields = extract_fields(event);

    if meta.target() == PROGRESS_TARGET {
        return progress(&fields);
    }

    let logger = fields
        .remove("logger")
        .map(|v| v.to_string().replace('"', ""));

    // An explicit MCP level (from `LogMessage::write`) preserves
    // severities tracing cannot express; otherwise map the tracing level.
    let level = fields
        .remove(MCP_LEVEL)
        .and_then(|v| serde_json::from_value::<LoggingLevel>(v).ok())
        .unwrap_or_else(|| meta.level().into());

    // The fields are this event's own, so they move into the message.
    let data = fields
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect::<serde_json::Map<_, _>>();

    let log = LogMessage {
        level,
        data: Some(serde_json::Value::Object(data)),
        logger,
    };

    Some(Notification::from(log))
}

/// The progress notification a `progress`-target event describes: a `token`,
/// a numeric `value`, and optionally a numeric `total`.
///
/// An event without a token or a value, or whose value is not a number,
/// describes no progress, and nothing is sent for it. A `total` that is not a
/// number is left out, and the progress it came with is still reported.
fn progress(fields: &BTreeMap<&str, serde_json::Value>) -> Option<Notification> {
    use serde::Deserialize as _;

    let token = ProgressToken::deserialize(fields.get("token")?).ok()?;
    let value = number(fields.get("value")?)?;
    let total = fields.get("total").and_then(number);

    Some(token.notify(value, total).into())
}

/// A number as an event field carries it: a JSON number, or the text of one,
/// which is how a float -- recorded through `Debug` -- arrives.
fn number(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim_matches('"').parse().ok(),
        _ => None,
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

/// What an event builds, in both profiles: the progress branch above all,
/// which used to `unwrap` its way through an event's fields.
#[cfg(test)]
mod build_tests {
    use super::build_notification;
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::prelude::*;

    /// Records what each event builds into.
    struct Built(Arc<Mutex<Vec<Option<Value>>>>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Built {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let built = build_notification(event)
                .map(|n| serde_json::to_value(n).expect("a notification serializes"));
            self.0.lock().expect("not poisoned").push(built);
        }
    }

    fn built(emit: impl FnOnce()) -> Vec<Option<Value>> {
        let out = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(Built(out.clone()));
        tracing::subscriber::with_default(subscriber, emit);
        out.lock().expect("not poisoned").clone()
    }

    /// Each of these panicked inside the layer -- that is, inside whatever
    /// handler emitted it. An event that describes no progress builds nothing.
    #[test]
    fn a_progress_event_that_describes_no_progress_builds_nothing() {
        let out = built(|| {
            tracing::info!(target: "progress", value = 1);
            tracing::info!(target: "progress", token = "t");
            tracing::info!(target: "progress", token = "t", value = "half");
            tracing::info!(target: "progress", token = "t", value = true);
        });

        assert_eq!(out, vec![None, None, None, None]);
    }

    #[test]
    fn a_progress_event_builds_a_progress_notification() {
        let out = built(|| {
            tracing::info!(target: "progress", token = "t", value = 0.5, total = 2);
            tracing::info!(target: "progress", token = 7, value = 1);
        });

        let half = out[0].as_ref().expect("a progress notification");
        assert_eq!(half["method"], "notifications/progress");
        assert_eq!(
            half["params"],
            json!({ "progressToken": "t", "progress": 0.5, "total": 2.0 })
        );

        let one = out[1].as_ref().expect("a progress notification");
        assert_eq!(
            one["params"],
            json!({ "progressToken": 7, "progress": 1.0 })
        );
    }

    /// The progress is what the event is about; a total that cannot be read
    /// is left out rather than taking the progress down with it.
    #[test]
    fn an_unreadable_total_is_left_out() {
        let out =
            built(|| tracing::info!(target: "progress", token = "t", value = 3, total = "lots"));

        let progress = out[0].as_ref().expect("a progress notification");
        assert_eq!(
            progress["params"],
            json!({ "progressToken": "t", "progress": 3.0 })
        );
    }

    /// Whether an event is delivered under the requested level, for each
    /// event, and how many times the level was asked for.
    #[cfg(not(feature = "legacy-spec"))]
    fn delivered(requested: Option<u8>, emit: impl FnOnce()) -> (Vec<bool>, usize) {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Delivered {
            requested: Option<u8>,
            asked: Arc<AtomicUsize>,
            out: Arc<Mutex<Vec<bool>>>,
        }

        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Delivered {
            fn on_event(
                &self,
                event: &tracing::Event<'_>,
                _: tracing_subscriber::layer::Context<'_, S>,
            ) {
                let delivered = super::delivered(event, || {
                    self.asked.fetch_add(1, Ordering::Relaxed);
                    self.requested
                });
                self.out.lock().expect("not poisoned").push(delivered);
            }
        }

        let asked = Arc::new(AtomicUsize::new(0));
        let out = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(Delivered {
            requested,
            asked: asked.clone(),
            out: out.clone(),
        });
        tracing::subscriber::with_default(subscriber, emit);
        let out = out.lock().expect("not poisoned").clone();
        (out, asked.load(Ordering::Relaxed))
    }

    /// Settled from the event before anything is built, by the same rules
    /// the built notification was filtered by: its MCP level where it states
    /// one, its tracing level otherwise, and progress never gated.
    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn delivery_is_settled_before_anything_is_built() {
        use crate::types::notification::LoggingLevel;

        let emit = || {
            tracing::error!("error");
            tracing::info!("info");
            // Tracing's `WARN`, but MCP's `notice`: below `warning`.
            tracing::warn!(mcp_level = "notice", "notice");
            // An MCP level that names none falls back to the tracing level.
            tracing::warn!(mcp_level = "bogus", "warning");
            tracing::info!(target: "progress", token = "t", value = 1);
        };

        let warning = Some(LoggingLevel::Warning.severity());
        let (out, asked) = delivered(warning, emit);
        assert_eq!(out, [true, false, false, true, true]);
        assert_eq!(asked, 4, "progress does not ask for the level");

        let (out, _) = delivered(None, emit);
        assert_eq!(out, [false, false, false, false, true]);
    }

    /// The fields move into the message; `logger` and the MCP level are taken
    /// out of them on the way.
    #[test]
    fn a_log_event_builds_a_log_message() {
        let out = built(|| tracing::warn!(logger = "tool", mcp_level = "notice", answer = 42));

        let log = out[0].as_ref().expect("a log message");
        assert_eq!(log["method"], "notifications/message");
        assert_eq!(log["params"]["level"], "notice");
        assert_eq!(log["params"]["logger"], "tool");
        assert_eq!(log["params"]["data"], json!({ "answer": 42 }));
    }
}

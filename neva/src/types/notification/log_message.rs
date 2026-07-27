//! Utilities for log messages

#[cfg(all(feature = "server", feature = "legacy-spec"))]
use crate::app::handler::{FromHandlerParams, HandlerParams};
use crate::error::Error;
use crate::types::notification::Notification;
use crate::types::response::ErrorDetails;
#[cfg(all(feature = "server", feature = "legacy-spec"))]
use crate::types::{FromRequest, Request};
use serde::{Deserialize, Serialize};
#[cfg(feature = "tracing")]
use tracing::Level;

/// The severity of a log message.
/// This map to syslog message severities, as specified in
/// [RFC-5424](https://datatracker.ietf.org/doc/html/rfc5424#section-6.2.1):
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoggingLevel {
    /// Detailed debug information, typically only valuable to developers.
    Debug,

    /// Normal operational messages that require no action.
    Info,

    /// Warning conditions that don't represent an error but indicate potential issues.
    Warning,

    /// Error conditions that should be addressed but don't require immediate action.
    Error,

    /// Normal but significant events that might deserve attention.
    Notice,

    /// Critical conditions that require immediate attention.
    Critical,

    /// Action must be taken immediately to address the condition.
    Alert,

    /// System is unusable and requires immediate attention.
    Emergency,
}

#[cfg(all(feature = "tracing", not(feature = "legacy-spec")))]
impl LoggingLevel {
    /// Severity rank where a higher number is more severe, following the
    /// RFC-5424 ordering. A message at `self` is delivered to a client that
    /// requested `min` when `self.severity() >= min.severity()`.
    ///
    /// The enum's declaration order is not the severity order, so this cannot
    /// be derived; it is also what travels on the request span as a plain
    /// integer field, avoiding a redundant string encoding of the level.
    #[inline]
    pub(crate) fn severity(self) -> u8 {
        match self {
            LoggingLevel::Debug => 0,
            LoggingLevel::Info => 1,
            LoggingLevel::Notice => 2,
            LoggingLevel::Warning => 3,
            LoggingLevel::Error => 4,
            LoggingLevel::Critical => 5,
            LoggingLevel::Alert => 6,
            LoggingLevel::Emergency => 7,
        }
    }
}

/// Sent from the server as the payload of "notifications/message" notifications whenever a log message is generated.
/// If no logging/setLevel request has been sent from the client, the server MAY decide which messages to send automatically.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogMessage {
    /// The severity of this log message.
    pub level: LoggingLevel,

    /// An optional name of the logger issuing this message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logger: Option<String>,

    /// The data to be logged, such as a string message or an object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// A request from the client to the server, to enable or adjust logging.
///
/// Removed under MCP 2026-07-28: the global `logging/setLevel` handshake is
/// gone; the desired level now rides per-request on
/// `_meta["io.modelcontextprotocol/logLevel"]`.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[cfg(feature = "legacy-spec")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetLevelRequestParams {
    /// The level of logging that the client wants to receive from the server.
    /// The server should send all logs at this level and higher (i.e., more severe) to the client as notifications/message.
    pub level: LoggingLevel,
}

impl From<Error> for LogMessage {
    #[inline]
    fn from(err: Error) -> Self {
        let details: ErrorDetails = err.into();
        Self {
            level: LoggingLevel::Error,
            logger: None,
            data: Some(serde_json::to_value(&details).unwrap()),
        }
    }
}

impl From<LogMessage> for Notification {
    #[inline]
    fn from(log: LogMessage) -> Self {
        Self::new(super::commands::MESSAGE, serde_json::to_value(log).ok())
    }
}

#[cfg(all(feature = "server", feature = "legacy-spec"))]
impl FromHandlerParams for SetLevelRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

impl LogMessage {
    /// Creates a new [`LogMessage`]
    #[inline]
    pub fn new(
        level: LoggingLevel,
        logger: Option<String>,
        data: Option<serde_json::Value>,
    ) -> Self {
        Self {
            level,
            logger,
            data,
        }
    }

    /// Writes a log message
    #[inline]
    #[cfg(feature = "tracing")]
    pub fn write(self) {
        let data = serde_json::to_value(&self.data).unwrap_or_default();
        // Carry the original MCP level as a field: tracing has only
        // ERROR/WARN/INFO/DEBUG, so Notice/Critical/Alert/Emergency would
        // otherwise be lost when the notification is rebuilt and filtered.
        let mcp_level = serde_json::to_value(self.level)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();

        let mcp_level = mcp_level.as_str();
        match self.level {
            LoggingLevel::Alert => tracing::event!(Level::ERROR, mcp_level, %data),
            LoggingLevel::Critical => tracing::event!(Level::ERROR, mcp_level, %data),
            LoggingLevel::Emergency => tracing::event!(Level::ERROR, mcp_level, %data),
            LoggingLevel::Error => tracing::event!(Level::ERROR, mcp_level, %data),
            LoggingLevel::Warning => tracing::event!(Level::WARN, mcp_level, %data),
            LoggingLevel::Notice => tracing::event!(Level::WARN, mcp_level, %data),
            LoggingLevel::Info => tracing::event!(Level::INFO, mcp_level, %data),
            LoggingLevel::Debug => tracing::event!(Level::DEBUG, mcp_level, %data),
        };
    }
}

#[cfg(all(test, feature = "tracing", not(feature = "legacy-spec")))]
mod tests {
    use super::LoggingLevel;

    const ALL: [LoggingLevel; 8] = [
        LoggingLevel::Debug,
        LoggingLevel::Info,
        LoggingLevel::Notice,
        LoggingLevel::Warning,
        LoggingLevel::Error,
        LoggingLevel::Critical,
        LoggingLevel::Alert,
        LoggingLevel::Emergency,
    ];

    #[test]
    fn severity_follows_rfc5424_ordering() {
        // Strictly increasing from least to most severe.
        for pair in ALL.windows(2) {
            assert!(
                pair[0].severity() < pair[1].severity(),
                "{:?} should be less severe than {:?}",
                pair[0],
                pair[1]
            );
        }
    }
}

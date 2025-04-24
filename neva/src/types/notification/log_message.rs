//! Utilities for log messages

use serde::{Serialize, Deserialize};
use crate::error::Error;
use crate::app::handler::{FromHandlerParams, HandlerParams};
use crate::types::{Request, FromRequest, response::ErrorDetails};
use crate::types::notification::Notification;
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
    Emergency
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
    pub logger: Option<String>,
    
    /// The data to be logged, such as a string message or an object.
    pub data: Option<serde_json::Value>,
}

/// A request from the client to the server, to enable or adjust logging.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
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
        Self::new(
            "notifications/message", 
            serde_json::to_value(log).ok()
        )
    }
}

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
        data: Option<serde_json::Value>
    ) -> Self {
        Self { level, logger, data }
    }
    
    /// Writes a log message
    #[inline]
    #[cfg(feature = "tracing")]
    pub fn write(self) {
        let data = serde_json::to_value(&self.data).unwrap_or_default();
        match self.level {
            LoggingLevel::Alert => tracing::event!(Level::ERROR, %data),
            LoggingLevel::Critical => tracing::event!(Level::ERROR, %data),
            LoggingLevel::Emergency => tracing::event!(Level::ERROR, %data),
            LoggingLevel::Error => tracing::event!(Level::ERROR, %data),
            LoggingLevel::Warning => tracing::event!(Level::WARN, %data),
            LoggingLevel::Notice => tracing::event!(Level::WARN, %data),
            LoggingLevel::Info => tracing::event!(Level::INFO, %data),
            LoggingLevel::Debug => tracing::event!(Level::DEBUG, %data),
        };
    }
}
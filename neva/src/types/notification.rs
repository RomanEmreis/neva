//! Utilities for Notifications

use serde::{Serialize, Deserialize};
use serde::de::DeserializeOwned;
use crate::types::{RequestId, Message, JSONRPC_VERSION};
#[cfg(feature = "server")]
use crate::{error::Error, types::{FromRequest, Request}};

pub use log_message::{
    LogMessage, 
    LoggingLevel, 
    SetLevelRequestParams
};

#[cfg(feature = "server")]
use crate::app::handler::{FromHandlerParams, HandlerParams};

pub use progress::ProgressNotification;

#[cfg(feature = "tracing")]
pub use formatter::NotificationFormatter;

mod progress;
mod log_message;
#[cfg(feature = "tracing")]
mod formatter;
#[cfg(feature = "tracing")]
pub mod fmt;

/// List of commands for Notifications
pub mod commands {
    /// Notification name that indicates that the notifications have initialized.
    pub const INITIALIZED: &str = "notifications/initialized";
    
    /// Notification name that indicates that notifications have been canceled.
    pub const CANCELLED: &str = "notifications/cancelled";
    
    /// Notification name that indicates that a new log message has been received.
    pub const MESSAGE: &str = "notifications/message";
    
    /// Notification name that indicates that a progress notification has been received.
    pub const PROGRESS: &str = "notifications/progress";
    
    /// Notification name that indicates that a log message has been received on stderr.
    pub const STDERR: &str = "notifications/stderr";
    
    /// Command name that sets the log level.
    pub const SET_LOG_LEVEL: &str = "logging/setLevel";
}

/// A notification which does not expect a response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    /// JSON-RPC protocol version. 
    ///
    /// > Note: always 2.0.
    pub jsonrpc: String,

    /// Name of the notification method.
    pub method: String,

    /// Optional parameters for the notifications.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,

    /// Current MCP Session ID
    #[serde(skip)]
    pub session_id: Option<uuid::Uuid>,
}

/// This notification can be sent by either side to indicate that it is cancelling 
/// a previously-issued request.
/// 
/// The request **SHOULD** still be in-flight, but due to communication latency, 
/// it is always possible that this notification **MAY** arrive after the request has already finished.
/// 
/// This notification indicates that the result will be unused, 
/// so any associated processing **SHOULD** cease.
/// 
/// A client **MUST NOT** attempt to cancel its `initialize` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelledNotificationParams {
    /// The ID of the request to cancel.
    /// 
    /// This **MUST** correspond to the ID of a request previously issued in the same direction.
    #[serde(rename = "requestId")]
    pub request_id: RequestId,
    
    /// An optional string describing the reason for the cancellation. 
    /// This **MAY** be logged or presented to the user.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl From<Notification> for Message {
    #[inline]
    fn from(notification: Notification) -> Self {
        Self::Notification(notification)
    }
}

#[cfg(feature = "server")]
impl FromHandlerParams for CancelledNotificationParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

impl Notification {
    /// Create a new [`Notification`]
    #[inline]
    pub fn new(method: &str, params: Option<serde_json::Value>) -> Self {
        Self { 
            jsonrpc: JSONRPC_VERSION.into(),
            session_id: None,
            method: method.into(), 
            params
        }
    }

    /// Returns the full id (session_id?/"(no_id)")
    pub fn full_id(&self) -> RequestId {
        let id = RequestId::default();
        if let Some(session_id) = self.session_id {
            id.concat(RequestId::Uuid(session_id))
        } else {
            id
        }
    }
    
    /// Parses [`Notification`] params into specified type
    #[inline]
    pub fn params<T: DeserializeOwned>(&self) -> Option<T> {
        match self.params { 
            Some(ref params) => serde_json::from_value(params.clone()).ok(),
            None => None,
        }
    }
    
    /// Writes the [`Notification`]
    #[inline]
    #[cfg(feature = "tracing")]
    pub fn write(self) {
        let is_stderr = self.is_stderr();
        let Some(params) = self.params else { return; };
        if is_stderr {
            Self::write_err_internal(params);
        } else {
            match serde_json::from_value::<LogMessage>(params.clone()) { 
                Ok(log) => log.write(),
                Err(err) => tracing::error!(logger = "neva", "{}", err),
            }
        }
    }
    
    /// Returns `true` is the [`Notification`] received with method `notifications/stderr`
    #[inline]
    pub fn is_stderr(&self) -> bool {
        self.method.as_str() == commands::STDERR
    }
    
    /// Writes the [`Notification`] as [`LoggingLevel::Error`]
    #[inline]
    #[cfg(feature = "tracing")]
    pub fn write_err(self) {
        if let Some(params) = self.params {
            Self::write_err_internal(params)
        }
    }
    
    /// Serializes the [`Notification`] to JSON string
    pub fn to_json(self) -> String {
        serde_json::to_string(&self).unwrap()
    }
    
    #[inline]
    #[cfg(feature = "tracing")]
    fn write_err_internal(params: serde_json::Value) {
        let err = params
            .get("content")
            .unwrap_or(&params);
        tracing::error!("{}", err);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use super::*;
    
    #[test]
    fn it_creates_new_notification() {
        let notification = Notification::new("test", Some(json!({ "param": "value" })));
        
        assert_eq!(notification.jsonrpc, "2.0");
        assert_eq!(notification.method, "test");
        
        let params_json = serde_json::to_string(&notification.params.unwrap()).unwrap();
        
        assert_eq!(params_json, r#"{"param":"value"}"#);
    }
}
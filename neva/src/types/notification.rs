//! Utilities for Notifications

use serde::{Serialize, Deserialize};
use crate::types::{FromRequest, Request, RequestId, JSONRPC_VERSION};
use crate::app::handler::{FromHandlerParams, HandlerParams};
use crate::error::Error;

pub use log_message::{
    LogMessage, 
    LoggingLevel, 
    SetLevelRequestParams
};

pub use progress::ProgressNotification;

#[cfg(feature = "tracing")]
pub use formatter::NotificationFormatter;

pub mod progress;
pub mod log_message;
#[cfg(feature = "tracing")]
pub mod formatter;

/// List of commands for Notifications
pub mod commands {
    pub const INITIALIZED: &str = "notifications/initialized";
    pub const CANCELLED: &str = "notifications/cancelled";
    pub const MESSAGE: &str = "notifications/message";
    pub const PROGRESS: &str = "notifications/progress";
    pub const STDERR: &str = "notifications/stderr";
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
            method: method.into(), 
            params
        }
    }
    
    /// Writes the [`Notification`]
    #[inline]
    pub fn write(self) {
        let is_stderr = self.is_stderr();
        let Some(params) = self.params else { return; };
        if is_stderr {
            Self::write_err_internal(params);
        } else {
            let log = serde_json::from_value::<LogMessage>(params).unwrap();
            log.write();
        }
    }
    
    /// Returns `true` is the [`Notification`] received with method `notifications/stderr`
    #[inline]
    pub fn is_stderr(&self) -> bool {
        self.method.as_str() == commands::STDERR
    }
    
    /// Writes the [`Notification`] as [`LoggingLevel::Error`]
    #[inline]
    pub fn write_err(self) {
        if let Some(params) = self.params {
            Self::write_err_internal(params)
        }
    }
    
    pub fn to_json(self) -> String {
        serde_json::to_string(&self).unwrap()
    }
    
    #[inline]
    fn write_err_internal(params: serde_json::Value) {
        let err = params
            .get("content")
            .unwrap_or_else(|| &params);
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
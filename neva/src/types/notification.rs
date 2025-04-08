//! Utilities for Notifications

use serde::{Serialize, Deserialize};
use crate::types::JSONRPC_VERSION;

pub use log_message::{LogMessage, LoggingLevel, SetLevelRequestParams};

pub mod log_message;

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
    pub params: Option<serde_json::Value>,
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
    
    /// Create a logging [`Notification`]
    #[inline]
    pub fn log(log: LogMessage) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            method: "notifications/message".into(),
            params: Some(serde_json::to_value(log).unwrap())
        }
    }
    
    pub fn to_json(self) -> String {
        serde_json::to_string(&self).unwrap()
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
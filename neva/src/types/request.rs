//! Represents a request from MCP client

use std::fmt;
use serde::{Serialize, Deserialize};

pub use from_request::FromRequest;

pub mod from_request;

/// A unique identifier for a request
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    String(String),
    Number(i64),
}

impl Default for RequestId {
    #[inline]
    fn default() -> RequestId {
        Self::String("(no id)".into())
    }
}

/// A request in the JSON-RPC protocol.
#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    /// JSON-RPC protocol version. 
    ///
    /// > Note: always 2.0.
    pub jsonrpc: String,
    
    /// Name of the method to invoke.
    pub method: String,
    
    /// Optional parameters for the method.
    pub params: Option<serde_json::Value>,
    
    /// Request identifier. Must be a string or number and unique within the session.
    pub id: Option<RequestId>,
}

impl fmt::Display for RequestId {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RequestId::String(str) => write!(f, "{}", str),
            RequestId::Number(num) => write!(f, "{}", num),
        }
    }
}

impl Request {
    /// Consumes the request and returns request's id if it's specified, otherwise returns default value
    /// 
    /// Default: `(no id)`
    pub fn into_id(self) -> RequestId {
        self.id
            .unwrap_or_default()
    }

    /// Returns request's id if it's specified, otherwise returns default value
    ///
    /// Default: `(no id)`
    pub fn id(&self) -> RequestId {
        self.id
            .clone()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {

}
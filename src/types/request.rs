//! Represents a request from MCP client

use std::fmt;
use serde::{Serialize, Deserialize};

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

#[cfg(test)]
mod tests {

}
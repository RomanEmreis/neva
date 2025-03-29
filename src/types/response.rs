//! Represents a response that MCP server provides

use serde::Serialize;
use serde_json::{json, Value};
pub use into_response::IntoResponse;
use crate::types::{
    RequestId,
    JSONRPC_VERSION
};

pub mod into_response;

/// A response message in the JSON-RPC protocol.
#[derive(Debug, Clone, Serialize)]
pub struct Response {
    /// JSON-RPC protocol version. 
    /// 
    /// > Note: always 2.0.
    pub jsonrpc: String,
    
    /// Request identifier matching the original request.
    pub id: RequestId,
    
    /// The result of the method invocation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    
    /// Error information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    /// Creates a successful response
    pub fn success(id: RequestId, result: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Creates a dummy successful response
    pub fn pong(id: RequestId) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: Some(json!({})),
            error: None,
        }
    }

    /// Creates an error response
    pub fn error(id: RequestId, error: &str) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: None,
            error: Some(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::types::RequestId;
    use super::Response;

    #[test]
    fn it_deserializes_successful_response_with_int_id_to_json() {
        let resp = Response::success(
            RequestId::Number(42),
            serde_json::json!({ "key": "test" }));
        
        let json = serde_json::to_string(&resp).unwrap();
        
        assert_eq!(json, r#"{"jsonrpc":"2.0","id":42,"result":{"key":"test"}}"#);
    }

    #[test]
    fn it_deserializes_error_response_with_string_id_to_json() {
        let resp = Response::error(
            RequestId::String("id".into()),
            "some error message");

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"id","error":"some error message"}"#);
    }
}

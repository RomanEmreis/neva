//! Represents a response that MCP server provides

use crate::error::Error;
use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
pub use error_details::ErrorDetails;
pub use into_response::IntoResponse;
use crate::types::{RequestId, JSONRPC_VERSION};

pub mod error_details;
pub mod into_response;

/// A response message in the JSON-RPC protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// JSON-RPC protocol version. 
    /// 
    /// > Note: always 2.0.
    pub jsonrpc: String,
    
    /// Request identifier matching the original request.
    #[serde(default)]
    pub id: RequestId,
    
    /// The result of the method invocation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    
    /// Error information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorDetails>,
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
    pub fn empty(id: RequestId) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: Some(json!({})),
            error: None,
        }
    }

    /// Creates an error response
    pub fn error(id: RequestId, error: Error) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: None,
            error: Some(error.into()),
        }
    }
    
    /// Unwraps the [`Response`] into either result of `T` or [`Error`]
    pub fn into_result<T: DeserializeOwned>(self) -> Result<T, Error> {
        match self.result {
            Some(result) => serde_json::from_value::<T>(result)
                .map_err(Into::into),
            None => {
                let error = self.error
                    .unwrap_or_default()
                    .into();
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{error::Error, types::RequestId};
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
            Error::new(-32603, "some error message"));

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"id","error":{"code":-32603,"message":"some error message","data":null}}"#);
    }
}

//! Represents a response that MCP server provides

use crate::error::Error;
use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
pub use error_details::ErrorDetails;
pub use into_response::IntoResponse;
use crate::types::{RequestId, Message, JSONRPC_VERSION};

pub mod error_details;
pub mod into_response;

/// A response message in the JSON-RPC protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Response {
    Ok(OkResponse),
    Err(ErrorResponse)
}

/// A successful response message in the JSON-RPC protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OkResponse {
    /// JSON-RPC protocol version. 
    /// 
    /// > Note: always 2.0.
    pub jsonrpc: String,
    
    /// Request identifier matching the original request.
    #[serde(default)]
    pub id: RequestId,
    
    /// The result of the method invocation.
    pub result: Value,

    /// Current MCP Session ID
    #[serde(skip)]
    pub session_id: Option<uuid::Uuid>,
}

/// A response to a request that indicates an error occurred.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    /// JSON-RPC protocol version. 
    ///
    /// > Note: always 2.0.
    pub jsonrpc: String,

    /// Request identifier matching the original request.
    #[serde(default)]
    pub id: RequestId,

    /// Error information.
    pub error: ErrorDetails,

    /// Current MCP Session ID
    #[serde(skip)]
    pub session_id: Option<uuid::Uuid>,
} 

impl From<Response> for Message {
    #[inline]
    fn from(response: Response) -> Self {
        Self::Response(response)
    }
}

impl Response {
    /// Creates a successful response
    pub fn success(id: RequestId, result: Value) -> Self {
        Response::Ok(OkResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            session_id: None,
            id,
            result
        })
    }

    /// Creates a dummy successful response
    pub fn empty(id: RequestId) -> Self {
        Response::Ok(OkResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            session_id: None,
            id,
            result: json!({})
        })
    }

    /// Creates an error response
    pub fn error(id: RequestId, error: Error) -> Self {
        Response::Err(ErrorResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            session_id: None,
            id,
            error: error.into(),
        })
    }
    
    /// Returns [`Response`] ID
    pub fn id(&self) -> &RequestId {
        match &self {
            Response::Ok(ok) => &ok.id,
            Response::Err(err) => &err.id
        }
    }
    
    /// Returns the full id (session_id?/response_id)
    pub fn full_id(&self) -> RequestId {
        let id = self.id();
        if let Some(session_id) = self.session_id() {
            RequestId::String(format!("{}/{}", session_id, id))
        } else {
            id.clone()
        }
    }
    
    /// Set the `id` for the response
    pub fn set_id(mut self, id: RequestId) -> Self {
        match &mut self {
            Response::Ok(ok) => ok.id = id,
            Response::Err(err) => err.id = id
        }
        self
    }

    /// Returns MCP Session ID
    #[inline]
    pub fn session_id(&self) -> Option<&uuid::Uuid> {
        match &self {
            Response::Ok(ok) => ok.session_id.as_ref(),
            Response::Err(err) => err.session_id.as_ref(),
        }
    }

    /// Set MCP `session_id` for the response
    pub fn set_session_id(mut self, id: uuid::Uuid) -> Self {
        match &mut self {
            Response::Ok(ok) => ok.session_id = Some(id),
            Response::Err(err) => err.session_id = Some(id)
        }
        self
    }
    
    /// Unwraps the [`Response`] into either result of `T` or [`Error`]
    pub fn into_result<T: DeserializeOwned>(self) -> Result<T, Error> {
        match self {
            Response::Ok(ok) => serde_json::from_value::<T>(ok.result).map_err(Into::into),
            Response::Err(err) => Err(err.error.into())
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

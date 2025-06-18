//! Represents a request from MCP client

use std::fmt;
use std::fmt::{Debug, Formatter};
use serde::{Serialize, Deserialize};
use super::{ProgressToken, Message, JSONRPC_VERSION};

#[cfg(feature = "server")]
use crate::Context;

pub use from_request::FromRequest;

pub mod from_request;

/// A unique identifier for a request
#[derive(Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    String(String),
    Number(i64),
}

impl Clone for RequestId {
    #[inline]
    fn clone(&self) -> Self {
        match self { 
            RequestId::Number(num) => RequestId::Number(*num),
            RequestId::String(string) => RequestId::String(string.clone()),
        }
    }
}

impl Default for RequestId {
    #[inline]
    fn default() -> RequestId {
        Self::String("(no id)".into())
    }
}

/// A request in the JSON-RPC protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// JSON-RPC protocol version. 
    ///
    /// > **Note:** always 2.0.
    pub jsonrpc: String,

    /// Request identifier. Must be a string or number and unique within the session.
    pub id: RequestId,
    
    /// Name of the method to invoke.
    pub method: String,
    
    /// Optional parameters for the method.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    
    /// Current MCP Session ID
    #[serde(skip)]
    pub session_id: Option<String>,
}

/// Provides metadata related to the request that provides additional protocol-level information.
/// 
/// > **Note:** This class contains properties that are used by the Model Context Protocol
/// > for features like progress tracking and other protocol-specific capabilities.
#[derive(Default, Clone, Deserialize, Serialize)]
pub struct RequestParamsMeta {
    /// An opaque token that will be attached to any subsequent progress notifications.
    /// 
    /// > **Note:** The receiver is not obligated to provide these notifications.
    #[serde(rename = "progressToken", skip_serializing_if = "Option::is_none")]
    pub progress_token: Option<ProgressToken>,
    
    /// MCP request context
    #[serde(skip)]
    #[cfg(feature = "server")]
    pub(crate) context: Option<Context>
}

impl Debug for RequestParamsMeta {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestParamsMeta")
            .field("progress_token", &self.progress_token)
            .finish()
    }
}

impl fmt::Display for RequestId {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            RequestId::String(str) => write!(f, "{}", str),
            RequestId::Number(num) => write!(f, "{}", num),
        }
    }
}

impl From<Request> for Message {
    #[inline]
    fn from(request: Request) -> Self {
        Self::Request(request)
    }
}

impl From<&RequestId> for ProgressToken {
    #[inline]
    fn from(id: &RequestId) -> ProgressToken {
        match id { 
            RequestId::Number(num) => ProgressToken::Number(*num),
            RequestId::String(str) => ProgressToken::String(str.clone()),
        }
    }
}

impl RequestParamsMeta {
    /// Creates a new [`RequestParamsMeta`] with [`ProgressToken`] for a specific [`RequestId`]
    pub fn new(id: &RequestId) -> Self {
        Self {
            progress_token: Some(ProgressToken::from(id)),
            #[cfg(feature = "server")]
            context: None
        }
    }
}

impl Request {
    /// Creates a new [`Request`]
    pub fn new<T: Serialize>(id: Option<RequestId>, method: &str, params: Option<T>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            session_id: None,
            id: id.unwrap_or_default(),
            method: method.into(),
            params: params.and_then(|p| serde_json::to_value(p).ok()),
        }
    }
    
    /// Consumes the request and returns request's id if it's specified, otherwise returns default value
    /// 
    /// Default: `(no id)`
    pub fn into_id(self) -> RequestId {
        self.id
    }

    /// Returns request's id if it's specified, otherwise returns default value
    ///
    /// Default: `(no id)`
    pub fn id(&self) -> RequestId {
        self.id.clone()
    }
    
    /// Returns [`Request`] params metadata
    pub fn meta(&self) -> Option<RequestParamsMeta> {
        self.params.as_ref()?
            .get("_meta")
            .cloned()
            .and_then(|meta| serde_json::from_value(meta).ok())
    }
}

#[cfg(test)]
mod tests {

}
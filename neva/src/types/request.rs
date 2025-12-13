//! Represents a request from an MCP client

use std::fmt;
use std::fmt::{Debug, Formatter};
use serde::{Serialize, Deserialize};
use super::{ProgressToken, Message, JSONRPC_VERSION};

#[cfg(feature = "server")]
use crate::Context;

#[cfg(feature = "http-server")]
use {
    crate::auth::DefaultClaims,
    volga::headers::HeaderMap
};

#[cfg(feature = "tasks")]
use crate::types::RelatedTaskMetadata;

#[cfg(feature = "server")]
pub use from_request::FromRequest;
pub use request_id::RequestId;

#[cfg(feature = "server")]
mod from_request;
mod request_id;

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
    pub session_id: Option<uuid::Uuid>,

    /// HTTP headers
    #[serde(skip)]
    #[cfg(feature = "http-server")]
    pub headers: HeaderMap,

    /// Authentication and Authorization claims
    #[serde(skip)]
    #[cfg(feature = "http-server")]
    pub claims: Option<Box<DefaultClaims>>,
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
    
    /// Represents metadata for associating messages with a task.
    /// 
    /// > **Note:** Include this in the _meta field under the key `io.modelcontextprotocol/related-task`.
    #[serde(rename = "io.modelcontextprotocol/related-task", skip_serializing_if = "Option::is_none")]
    #[cfg(feature = "tasks")]
    pub(crate) task: Option<RelatedTaskMetadata>,

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

impl From<Request> for Message {
    #[inline]
    fn from(request: Request) -> Self {
        Self::Request(request)
    }
}

impl RequestParamsMeta {
    /// Creates a new [`RequestParamsMeta`] with [`ProgressToken`] for a specific [`RequestId`]
    pub fn new(id: &RequestId) -> Self {
        Self {
            progress_token: Some(ProgressToken::from(id)),
            #[cfg(feature = "tasks")]
            task: None,
            #[cfg(feature = "server")]
            context: None
        }
    }
}

impl Request {
    /// Creates a new [`Request`]
    pub fn new<T: Serialize>(id: Option<RequestId>, method: impl Into<String>, params: Option<T>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            session_id: None,
            id: id.unwrap_or_default(),
            method: method.into(),
            params: params.and_then(|p| serde_json::to_value(p).ok()),
            #[cfg(feature = "http-server")]
            headers: HeaderMap::with_capacity(8),
            #[cfg(feature = "http-server")]
            claims: None,
        }
    }

    /// Returns request's id if it's specified, otherwise returns default value
    ///
    /// Default: `(no id)`
    pub fn id(&self) -> RequestId {
        self.id.clone()
    }

    /// Returns the full id (session_id?/request_id)
    pub fn full_id(&self) -> RequestId {
        let id = self.id.clone();
        if let Some(session_id) = self.session_id {
            id.concat(RequestId::Uuid(session_id))
        } else {
            id
        }
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
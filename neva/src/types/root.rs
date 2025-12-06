//! Represents MCP Roots.

use serde::{Serialize, Deserialize};
use crate::types::{Uri, request::RequestParamsMeta, IntoResponse, RequestId, Response};

/// List of commands for Roots
pub mod commands {
    /// Command name that requests a list of roots available from the client.
    pub const LIST: &str = "roots/list";
    
    /// Notification name that indicates that the list of roots has changed.
    pub const LIST_CHANGED: &str = "notifications/roots/list_changed";
}

/// Represents a root URI and its metadata in the Model Context Protocol.
///
/// > **Note:** Root URIs serve as entry points for resource navigation, typically representing
/// > top-level directories or container resources that can be accessed and traversed.
/// > Roots provide a hierarchical structure for organizing and accessing resources within the protocol.
/// > Each root has a URI that uniquely identifies it and optional metadata like a human-readable name.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Root {
    /// The URI of the root.
    pub uri: Uri,
    
    /// A human-readable name for the root.
    pub name: String,

    /// Metadata reserved by MCP for protocol-level metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

/// Represents the parameters used to request a list of roots available from the client.
/// 
/// > **Note:** The client responds with a ['ListRootsResult'] containing the client's roots.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ListRootsRequestParams {
    /// Metadata related to the request that provides additional protocol-level information.
    ///
    /// > **Note:** This can include progress tracking tokens and other protocol-specific properties
    /// > that are not part of the primary request parameters.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<RequestParamsMeta>,
}

/// Represents the client's response to a `roots/list` request from the server.
/// This result contains an array of Root objects, each representing a root directory 
/// or file that the server can operate on.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ListRootsResult {
    /// The list of root URIs provided by the client.
    ///
    /// > **Note:** This collection contains all available root URIs and their associated metadata.
    /// > Each root serves as an entry point for resource navigation in the Model Context Protocol.
    pub roots: Vec<Root>,
    
    /// An additional metadata for the result.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<RequestParamsMeta>,
}

impl IntoResponse for ListRootsResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        match serde_json::to_value(self) {
            Ok(v) => Response::success(req_id, v),
            Err(err) => Response::error(req_id, err.into())
        }
    }
}

impl From<Vec<Root>> for ListRootsResult {
    #[inline]
    fn from(roots: Vec<Root>) -> Self {
        Self {
            roots,
            meta: None,
        }
    }
}

impl<U, N> From<(U, N)> for Root
where
    U: Into<Uri>,
    N: Into<String>,
{
    #[inline]
    fn from(parts: (U, N)) -> Self {
        let (uri, name) = parts;
        Self::new(uri, name)
    }
}

impl Root {
    /// Creates a new [`Root`]
    pub fn new(uri: impl Into<Uri>, name: impl Into<String>) -> Self {
        Self { 
            uri: uri.into(), 
            name: name.into(),
            meta: None,
        }
    }
    
    /// Split [`Root`] into parts of URI and name
    pub fn into_parts(self) -> (Uri, String) {
        (self.uri, self.name)
    }
}

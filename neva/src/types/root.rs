//! Represents MCP Roots.

use serde::{Serialize, Deserialize};
use crate::types::{Uri, request::RequestParamsMeta, IntoResponse, RequestId, Response};

/// List of commands for Roots
pub mod commands {
    pub const LIST: &str = "roots/list";
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
    pub name: String
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

#[derive(Default, Serialize, Deserialize)]
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
        Response::success(req_id, serde_json::to_value(self).unwrap())
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

impl Root {
    /// Creates a new [`Root`]
    pub fn new(uri: &str, name: &str) -> Self {
        Self { 
            uri: Uri::from(uri.to_string()), 
            name: name.into()
        }
    }
}

//! Represents an MCP resource

use serde::{Deserialize, Serialize};
use crate::types::{IntoResponse, RequestId, Response};

/// Represents a known resource that the server is capable of reading.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Serialize)]
pub struct Resource {
    /// The URI of this resource.
    pub uri: String,
    
    /// A human-readable name for this resource.
    pub name: String,

    /// A description of what this resource represents.
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,

    /// The MIME type of this resource, if known.
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
}

/// Sent from the client to the server, to read a specific resource URI.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Deserialize)]
pub struct ReadResourceRequestParams {
    /// The URI of the resource to read. The URI can use any protocol; 
    /// it is up to the server how to interpret it.
    pub uri: String,
}

/// The server's response to a resources/read request from the client.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Serialize)]
pub struct ReadResourceResult {
    /// A list of ResourceContents that this resource contains.
    pub contents: Vec<ResourceContents>
}

/// Represents the content of a resource.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Serialize)]
pub struct ResourceContents {
    /// The URI of the resource.
    pub uri: String,

    /// The type of content.
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,

    /// The text content of the resource.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    /// The base64-encoded binary content of the resource.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>
}

/// The server's response to a resources/list request from the client.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Default, Serialize)]
pub struct ListResourcesResult<'a> {
    /// A list of resources that the server offers.
    pub resources: Vec<&'a Resource>
}

impl IntoResponse for ListResourcesResult<'_> {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        Response::success(req_id, serde_json::to_value(self).unwrap())
    }
}

impl<'a> From<Vec<&'a Resource>> for ListResourcesResult<'a> {
    #[inline]
    fn from(resources: Vec<&'a Resource>) -> Self {
        Self { resources }
    }
}

impl ListResourcesResult<'_> {
    /// Create a new [`ListResourcesResult`]
    #[inline]
    pub fn new() -> Self {
        Default::default()
    }
}
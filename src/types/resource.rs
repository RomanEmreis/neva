//! Represents an MCP resource

use serde::{Deserialize, Serialize};
use crate::types::{RequestId, Response, IntoResponse, Request};

pub use uri::Uri;
pub use read_resource_result::{ReadResourceResult, ResourceContents};
pub use template::{ResourceTemplate, ListResourceTemplatesResult, ListResourceTemplatesRequestParams};
pub(crate) use route::Route;
use crate::app::handler::{FromHandlerParams, HandlerParams};
use crate::error::Error;
use crate::types::request::FromRequest;

mod from_request;
pub mod read_resource_result;
pub mod uri;
pub mod template;
pub(crate) mod route;

/// Represents a known resource that the server is capable of reading.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Clone, Serialize)]
pub struct Resource {
    /// The URI of this resource.
    pub uri: Uri,
    
    /// A human-readable name for this resource.
    pub name: String,

    /// A description of what this resource represents.
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,

    /// The MIME type of this resource, if known.
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>
}

/// Sent from the client to request a list of resources the server has.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Deserialize)]
pub struct ListResourcesRequestParams {
    /// An opaque token representing the current pagination position.
    /// If provided, the server should return results starting after this cursor.
    pub cursor: Option<String>,
}

/// Sent from the client to the server, to read a specific resource URI.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Deserialize)]
pub struct ReadResourceRequestParams {
    /// The URI of the resource to read. The URI can use any protocol; 
    /// it is up to the server how to interpret it.
    pub uri: Uri,
}

/// The server's response to a resources/list request from the client.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Default, Serialize)]
pub struct ListResourcesResult {
    /// A list of resources that the server offers.
    pub resources: Vec<Resource>
}

/// Sent from the client to request resources/updated notifications 
/// from the server whenever a particular resource changes.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Deserialize)]
pub struct SubscribeRequestParams {
    /// The URI of the resource to subscribe to. 
    /// The URI can use any protocol; it is up to the server how to interpret it.
    pub uri: String,
}

/// Sent from the client to request not receiving updated notifications 
/// from the server whenever a primitive resource changes.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Deserialize)]
pub struct UnsubscribeRequestParams {
    /// The URI of the resource to unsubscribe from. 
    /// The URI can use any protocol; it is up to the server how to interpret it. 
    pub uri: String,
}

impl IntoResponse for ListResourcesResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        Response::success(req_id, serde_json::to_value(self).unwrap())
    }
}

impl<const N: usize> From<[Resource; N]> for ListResourcesResult {
    #[inline]
    fn from(resources: [Resource; N]) -> Self {
        Self { resources: resources.to_vec() }
    }
}

impl From<Vec<Resource>> for ListResourcesResult {
    #[inline]
    fn from(resources: Vec<Resource>) -> Self {
        Self { resources }
    }
}

impl FromHandlerParams for ListResourcesRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

impl FromHandlerParams for ReadResourceRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

impl FromHandlerParams for SubscribeRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

impl FromHandlerParams for UnsubscribeRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

impl ListResourcesResult {
    /// Creates a new [`ListResourcesResult`]
    #[inline]
    pub fn new() -> Self {
        Default::default()
    }
}

impl Resource {
    /// Creates a new [`Resource`]
    #[inline]
    pub fn new(uri: &'static str, name: &str) -> Self {
        Self { 
            uri: uri.into(), 
            name: name.into(), 
            descr: None, 
            mime: None,
        }
    }

    /// Sets a description for a resource
    pub fn with_description(mut self, description: &str) -> Self {
        self.descr = Some(description.into());
        self
    }

    /// Sets a MIME type for a resource
    pub fn with_mime(mut self, mime: &str) -> Self {
        self.mime = Some(mime.into());
        self
    }
}

#[cfg(test)]
mod tests {
    
}
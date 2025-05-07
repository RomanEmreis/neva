//! Represents an MCP resource

use serde::{Deserialize, Serialize};
use crate::types::Cursor;
use crate::types::request::RequestParamsMeta;
#[cfg(feature = "server")]
use crate::error::Error;
#[cfg(feature = "server")]
use crate::types::request::FromRequest;
#[cfg(feature = "server")]
use crate::types::{RequestId, Response, IntoResponse, Request, Page};
#[cfg(feature = "server")]
use crate::app::{context::Context, handler::{FromHandlerParams, HandlerParams}};

pub use uri::Uri;
pub use read_resource_result::{ReadResourceResult, ResourceContents};
pub use template::{ResourceTemplate, ListResourceTemplatesResult, ListResourceTemplatesRequestParams};

#[cfg(feature = "server")]
pub(crate) use route::Route;

pub mod read_resource_result;
pub mod uri;
pub mod template;
#[cfg(feature = "server")]
pub(crate) mod route;
#[cfg(feature = "server")]
mod from_request;

/// List of commands for Resources
pub mod commands {
    pub const LIST: &str = "resources/list";
    pub const LIST_CHANGED: &str = "notifications/resources/list_changed";
    pub const TEMPLATES_LIST: &str = "resources/templates/list";
    pub const READ: &str = "resources/read";
    pub const SUBSCRIBE: &str = "resources/subscribe";
    pub const UNSUBSCRIBE: &str = "resources/unsubscribe";
    pub const UPDATED: &str = "notifications/resources/updated";
}

/// Represents a known resource that the server is capable of reading.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Clone, Serialize, Deserialize)]
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
    pub mime: Option<String>,

    /// The resource size in bytes, if known
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<usize>
}

/// Sent from the client to request a list of resources the server has.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Serialize, Deserialize)]
pub struct ListResourcesRequestParams {
    /// An opaque token representing the current pagination position.
    /// If provided, the server should return results starting after this cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
}

/// Sent from the client to the server, to read a specific resource URI.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Serialize, Deserialize)]
pub struct ReadResourceRequestParams {
    /// The URI of the resource to read. The URI can use any protocol; 
    /// it is up to the server how to interpret it.
    pub uri: Uri,

    /// Metadata related to the request that provides additional protocol-level information.
    ///
    /// > **Note:** This can include progress tracking tokens and other protocol-specific properties
    /// > that are not part of the primary request parameters.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<RequestParamsMeta>,
}

/// The server's response to a resources/list request from the client.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Default, Serialize, Deserialize)]
pub struct ListResourcesResult {
    /// A list of resources that the server offers.
    pub resources: Vec<Resource>,

    /// An opaque token representing the pagination position after the last returned result.
    ///
    /// When a paginated result has more data available, the `next_cursor`
    /// field will contain `Some` token that can be used in subsequent requests
    /// to fetch the next page. When there are no more results to return, the `next_cursor` field
    /// will be `None`.
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

/// Sent from the client to request resources/updated notifications 
/// from the server whenever a particular resource changes.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Serialize, Deserialize)]
pub struct SubscribeRequestParams {
    /// The URI of the resource to subscribe to. 
    /// The URI can use any protocol; it is up to the server how to interpret it.
    pub uri: Uri,
}

/// Sent from the client to request not receiving updated notifications 
/// from the server whenever a primitive resource changes.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Serialize, Deserialize)]
pub struct UnsubscribeRequestParams {
    /// The URI of the resource to unsubscribe from. 
    /// The URI can use any protocol; it is up to the server how to interpret it. 
    pub uri: Uri,
}

impl<T: Into<Uri>> From<T> for SubscribeRequestParams {
    #[inline]
    fn from(uri: T) -> Self {
        Self { uri: uri.into() }
    }
}

impl<T: Into<Uri>> From<T> for UnsubscribeRequestParams {
    #[inline]
    fn from(uri: T) -> Self {
        Self { uri: uri.into() }
    }
}

#[cfg(feature = "server")]
impl IntoResponse for ListResourcesResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        Response::success(req_id, serde_json::to_value(self).unwrap())
    }
}

#[cfg(feature = "server")]
impl<const N: usize> From<[Resource; N]> for ListResourcesResult {
    #[inline]
    fn from(resources: [Resource; N]) -> Self {
        Self {
            next_cursor: None,
            resources: resources.to_vec()
        }
    }
}

#[cfg(feature = "server")]
impl From<Vec<Resource>> for ListResourcesResult {
    #[inline]
    fn from(resources: Vec<Resource>) -> Self {
        Self {
            next_cursor: None,
            resources
        }
    }
}

#[cfg(feature = "server")]
impl From<Page<'_, Resource>> for ListResourcesResult {
    #[inline]
    fn from(page: Page<Resource>) -> Self {
        Self {
            next_cursor: page.next_cursor,
            resources: page.items.to_vec()
        }
    }
}

#[cfg(feature = "server")]
impl FromHandlerParams for ListResourcesRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

#[cfg(feature = "server")]
impl FromHandlerParams for ReadResourceRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

#[cfg(feature = "server")]
impl FromHandlerParams for SubscribeRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

#[cfg(feature = "server")]
impl FromHandlerParams for UnsubscribeRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

#[cfg(feature = "server")]
impl ListResourcesResult {
    /// Creates a new [`ListResourcesResult`]
    #[inline]
    pub fn new() -> Self {
        Default::default()
    }
}

impl From<Uri> for Resource {
    #[inline]
    fn from(uri: Uri) -> Self {
        Self {
            name: uri.to_string(),
            descr: None,
            mime: None,
            size: None,
            uri
        }
    }
}

impl From<String> for Resource {
    #[inline]
    fn from(uri: String) -> Self {
        Self {
            name: uri.clone(),
            uri: uri.into(),
            descr: None,
            mime: None,
            size: None,
        }
    }
}

impl From<Uri> for ReadResourceRequestParams {
    #[inline]
    fn from(uri: Uri) -> Self {
        Self {
            meta: None,
            uri
        }
    }
}

#[cfg(feature = "server")]
impl ReadResourceRequestParams {
    /// Includes [`Context`] into request metadata. If metadata is `None` it creates a new.
    pub(crate) fn with_context(mut self, ctx: Context) -> Self {
        self.meta.get_or_insert_default().context = Some(ctx);
        self
    }
}

#[cfg(feature = "server")]
impl Resource {
    /// Creates a new [`Resource`]
    #[inline]
    pub fn new<U: Into<Uri>, S: Into<String>>(uri: U, name: S) -> Self {
        Self { 
            uri: uri.into(), 
            name: name.into(), 
            descr: None, 
            mime: None,
            size: None,
        }
    }

    /// Sets a description for a resource
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.descr = Some(description.into());
        self
    }

    /// Sets a MIME type for a resource
    pub fn with_mime(mut self, mime: impl Into<String>) -> Self {
        self.mime = Some(mime.into());
        self
    }

    /// Sets a resource size
    pub fn with_size(mut self, size: usize) -> Self {
        self.size = Some(size);
        self
    }
}

#[cfg(test)]
mod tests {
    
}
//! Utilities for Resource templates

use serde::{Deserialize, Serialize};
#[cfg(feature = "server")]
use std::sync::Arc;
#[cfg(feature = "server")]
use futures_util::future::BoxFuture;
#[cfg(feature = "server")]
use crate::error::Error;
#[cfg(feature = "server")]
use crate::app::handler::{
    FromHandlerParams, 
    GenericHandler, 
    Handler, 
    HandlerParams
};
use crate::types::{
    resource::Uri, Annotations, IntoResponse, 
    RequestId, Response, 
    Cursor, Page
};

#[cfg(feature = "server")]
use crate::types::{FromRequest, ReadResourceRequestParams, ReadResourceResult, Request};

/// Represents a known resource template that the server is capable of reading.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Clone, Serialize, Deserialize)]
pub struct ResourceTemplate {
    #[serde(rename = "uriTemplate")]
    pub uri_template: Uri,
    
    /// A human-readable name for this resource template.
    pub name: String,

    /// A description of what this resource template represents.
    pub descr: Option<String>,

    /// The MIME type of this resource template, if known.
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,

    /// Optional annotations for the resource template.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>
}

/// Sent from the client to request a list of resource templates the server has.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Serialize, Deserialize)]
pub struct ListResourceTemplatesRequestParams {
    /// An opaque token representing the current pagination position.
    /// If provided, the server should return results starting after this cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
}

/// The server's response to a resources/templates/list request from the client.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Default, Serialize, Deserialize)]
pub struct ListResourceTemplatesResult {
    /// A list of resource templates that the server offers.
    #[serde(rename = "resourceTemplates")]
    pub templates: Vec<ResourceTemplate>,

    /// An opaque token representing the pagination position after the last returned result.
    ///
    /// When a paginated result has more data available, the `next_cursor`
    /// field will contain `Some` token that can be used in subsequent requests
    /// to fetch the next page. When there are no more results to return, the `next_cursor` field
    /// will be `None`.
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

impl IntoResponse for ListResourceTemplatesResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        Response::success(req_id, serde_json::to_value(self).unwrap())
    }
}

impl From<Vec<ResourceTemplate>> for ListResourceTemplatesResult {
    #[inline]
    fn from(templates: Vec<ResourceTemplate>) -> Self {
        Self { 
            next_cursor: None,
            templates
        }
    }
}

impl From<Page<'_, ResourceTemplate>> for ListResourceTemplatesResult {
    #[inline]
    fn from(page: Page<ResourceTemplate>) -> Self {
        Self {
            next_cursor: page.next_cursor,
            templates: page.items.to_vec()
        }
    }
}

#[cfg(feature = "server")]
impl FromHandlerParams for ListResourceTemplatesRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

impl ListResourceTemplatesResult {
    /// Creates a new [`ListResourceTemplatesResult`]
    #[inline]
    pub fn new() -> Self {
        Default::default()
    }
}

/// Represents a function that reads a resource
#[cfg(feature = "server")]
pub(crate) struct ResourceFunc<F, R, Args>
where
    F: GenericHandler<Args, Output = R>,
    R: TryInto<ReadResourceResult>,
    Args: TryFrom<ReadResourceRequestParams, Error = Error>
{
    func: F,
    _marker: std::marker::PhantomData<Args>,
}

#[cfg(feature = "server")]
impl<F, R ,Args> ResourceFunc<F, R, Args>
where
    F: GenericHandler<Args, Output = R>,
    R: TryInto<ReadResourceResult>,
    Args: TryFrom<ReadResourceRequestParams, Error = Error>
{
    /// Creates a new [`ResourceFunc`] wrapped into [`Arc`]
    pub(crate) fn new(func: F) -> Arc<Self> {
        let func = Self { func, _marker: std::marker::PhantomData };
        Arc::new(func)
    }
}

#[cfg(feature = "server")]
impl<F, R ,Args> Handler<ReadResourceResult> for ResourceFunc<F, R, Args>
where
    F: GenericHandler<Args, Output = R>,
    R: TryInto<ReadResourceResult>,
    R::Error: Into<Error>,
    Args: TryFrom<ReadResourceRequestParams, Error = Error> + Send + Sync,
{
    #[inline]
    fn call(&self, params: HandlerParams) -> BoxFuture<Result<ReadResourceResult, Error>> {
        let HandlerParams::Resource(params) = params else {
            unreachable!()
        };
        Box::pin(async move {
            let args = Args::try_from(params)?;
            self.func
                .call(args)
                .await
                .try_into()
                .map_err(Into::into)
        })
    }
}

#[cfg(feature = "server")]
impl ResourceTemplate {
    /// Creates a new [`ResourceTemplate`]
    #[inline]
    pub fn new<U: Into<Uri>, S: Into<String>>(uri: U, name: S) -> Self {
        Self {
            uri_template: uri.into(),
            name: name.into(),
            mime: None,
            descr: None,
            annotations: None
        }
    }

    /// Sets a description for a resource template
    pub fn with_description(&mut self, description: &str) -> &mut Self {
        self.descr = Some(description.into());
        self
    }

    /// Sets a MIME type for all matching resources
    pub fn with_mime(&mut self, mime: &str) -> &mut Self {
        self.mime = Some(mime.into());
        self
    }
    
    /// Sets annotations for the resource template
    pub fn with_annotations<F>(&mut self, config: F) -> &mut Self
    where
        F: FnOnce(Annotations) -> Annotations 
    {
        self.annotations = Some(config(Default::default()));
        self
    }
}

#[cfg(test)]
mod tests {
    
}

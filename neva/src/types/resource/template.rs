//! Utilities for Resource templates

use std::sync::Arc;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use crate::error::Error;
use crate::app::handler::{
    FromHandlerParams, 
    GenericHandler, 
    Handler, 
    HandlerParams
};
use crate::types::{
    resource::Uri, 
    Annotations, 
    IntoResponse, 
    ReadResourceRequestParams, ReadResourceResult, 
    Request, RequestId, FromRequest,
    Response
};

/// Represents a known resource template that the server is capable of reading.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Clone, Serialize)]
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
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Deserialize)]
pub struct ListResourceTemplatesRequestParams {
    /// An opaque token representing the current pagination position.
    /// If provided, the server should return results starting after this cursor.
    pub cursor: Option<String>,
}

/// The server's response to a resources/templates/list request from the client.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Default, Serialize)]
pub struct ListResourceTemplatesResult {
    /// A list of resource templates that the server offers.
    #[serde(rename = "resourceTemplates")]
    pub templates: Vec<ResourceTemplate>,
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
        Self { templates }
    }
}

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
pub(crate) struct ResourceFunc<F, R, Args>
where
    F: GenericHandler<Args, Output = R>,
    R: TryInto<ReadResourceResult>,
    Args: TryFrom<ReadResourceRequestParams, Error = Error>
{
    func: F,
    _marker: std::marker::PhantomData<Args>,
}

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

impl ResourceTemplate {
    /// Creates a new [`ResourceTemplate`]
    #[inline]
    pub fn new(uri: &'static str, name: &str) -> Self {
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

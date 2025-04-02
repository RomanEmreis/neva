//! Completion request types

use serde::{Deserialize, Serialize};
use super::{IntoResponse, RequestId, Response, Reference, Request};
use crate::app::handler::{FromHandlerParams, HandlerParams};
use crate::error::Error;
use crate::types::request::FromRequest;

/// Represents a completion object in the server's response
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Serialize)]
pub struct Completion {
    /// An array of completion values. Must not exceed 100 items.
    pub values: Vec<String>,
    
    /// The total number of completion options available. 
    /// This can exceed the number of values actually sent in the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i32>,
    
    /// Indicates whether there are additional completion options beyond those provided
    /// in the current response, even if the exact total is unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
}

/// A request from the client to the server, to ask for completion options.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Deserialize)]
pub struct CompleteRequestParams {
    /// The reference's information
    #[serde(rename = "ref")]
    pub r#ref: Reference,
    
    /// The argument's information
    #[serde(rename = "argument")]
    pub arg: Argument,
}

/// Used for completion requests to provide additional context for the completion options.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Deserialize)]
pub struct Argument {
    /// The name of the argument.
    pub name: String,
    
    /// The value of the argument to use for completion matching.
    pub value: String,
}

/// The server's response to a completion/complete request
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Default, Serialize)]
pub struct CompleteResult {
    /// The completion object containing the completion values.
    pub completion: Completion,
}

impl Default for Completion {
    #[inline]
    fn default() -> Self {
        Self {
            values: vec![],
            total: Some(0),
            has_more: Some(false),
        }
    }
}

impl FromHandlerParams for CompleteRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

impl Completion {
    /// Creates a new empty [`Completion`] object
    #[inline]
    pub fn new() -> Self {
        Self {
            values: vec![],
            total: None,
            has_more: None
        }
    }
}

impl CompleteResult {
    /// Create a new [`CompleteResult`] object
    #[inline]
    pub fn new() -> Self {
        Self { completion: Completion::new() }
    }
}

impl IntoResponse for CompleteResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        Response::success(req_id, serde_json::to_value(self).unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn it_creates_default_completion() {
        let completion = Completion::default();
        
        assert_eq!(completion.values.len(), 0);
        assert_eq!(completion.total, Some(0));
        assert_eq!(completion.has_more, Some(false));
    }

    #[test]
    fn it_creates_new_completion() {
        let completion = Completion::new();

        assert_eq!(completion.values.len(), 0);
        assert_eq!(completion.total, None);
        assert_eq!(completion.has_more, None);
    }
    
    #[test]
    fn it_converts_complete_result_into_response() {
        let result = CompleteResult::default();
        
        let resp = result.into_response(RequestId::default());
        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"completion":{"has_more":false,"total":0,"values":[]}}}"#);
    }
}
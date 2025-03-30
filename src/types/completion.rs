//! Completion request types

use super::{IntoResponse, RequestId, Response};
use serde::Serialize;

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
        
    }
}
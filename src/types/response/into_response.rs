//! Tools for converting any type into MCP server response

use crate::types::{RequestId, Response};

/// A trait for converting any return type into MCP response
pub trait IntoResponse {
    /// Converts a type into MCP server response
    fn into_response(self, req_id: RequestId) -> Response;
}

impl IntoResponse for String {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        let result = serde_json::json!({ "result": self });
        Response::success(req_id, Some(result))
    }
}

impl IntoResponse for &'static str {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        let result = serde_json::json!({ "result": self });
        Response::success(req_id, Some(result))
    }
}

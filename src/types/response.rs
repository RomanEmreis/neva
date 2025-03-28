//! Represents a response that MCP server provides

use serde::Serialize;
use crate::types::JSONRPC_VERSION;

#[derive(Debug, Clone, Serialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn success(id: i32, result: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result,
            error: None,
        }
    }

    pub fn error(id: i32, error: String) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

impl From<String> for Response {
    fn from(str: String) -> Self {
        let result = serde_json::json!({ "result": str });
        Response::success(2, Some(result))
    }
}

impl From<&'static str> for Response {
    fn from(str: &str) -> Self {
        let result = serde_json::json!({ "result": str });
        Response::success(2, Some(result))
    }
}
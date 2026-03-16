//! Utilities for extraction params from Request

use crate::error::{Error, ErrorCode};
use crate::types::Request;
use serde::de::DeserializeOwned;

/// A trait that helps the extract typed _params_ from request
pub trait FromRequest: Sized {
    /// Extracts typed _params_ from request
    fn from_request(request: Request) -> Result<Self, Error>;
}

impl<T: DeserializeOwned> FromRequest for T {
    fn from_request(req: Request) -> Result<Self, Error> {
        let params = req.params.unwrap_or_else(|| serde_json::json!({}));
        serde_json::from_value(params).map_err(|e| Error::new(ErrorCode::InvalidParams, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;
    use crate::types::{
        Cursor,
        tool::{CallToolRequestParams, ListToolsRequestParams},
    };

    fn make_request(params: Option<serde_json::Value>) -> Request {
        Request::new(None::<crate::types::RequestId>, "test/method", params)
    }

    // --- ListToolsRequestParams (all-optional params) ---

    #[test]
    fn it_returns_defaults_when_params_absent_for_optional_params_type() {
        let req = make_request(None);
        let result = ListToolsRequestParams::from_request(req).unwrap();
        assert!(result.cursor.is_none());
    }

    #[test]
    fn it_returns_defaults_when_params_empty_object_for_optional_params_type() {
        let req = make_request(Some(serde_json::json!({})));
        let result = ListToolsRequestParams::from_request(req).unwrap();
        assert!(result.cursor.is_none());
    }

    #[test]
    fn it_deserializes_optional_params_with_cursor_present() {
        let cursor = Cursor(5);
        let req = make_request(Some(serde_json::json!({
            "cursor": serde_json::to_value(cursor).unwrap()
        })));
        let result = ListToolsRequestParams::from_request(req).unwrap();
        assert_eq!(result.cursor, Some(cursor));
    }

    // --- CallToolRequestParams (has required fields) ---

    #[test]
    fn it_errors_when_params_absent_for_required_params_type() {
        let req = make_request(None);
        let err = CallToolRequestParams::from_request(req).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[test]
    fn it_errors_when_required_field_missing_in_params() {
        let req = make_request(Some(serde_json::json!({})));
        let err = CallToolRequestParams::from_request(req).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[test]
    fn it_deserializes_required_params_when_present() {
        let req = make_request(Some(serde_json::json!({"name": "my_tool"})));
        let result = CallToolRequestParams::from_request(req).unwrap();
        assert_eq!(result.name, "my_tool");
        assert!(result.args.is_none());
    }
}

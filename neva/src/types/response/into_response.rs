//! Tools for converting any type into MCP server response

use serde::Serialize;
use crate::error::{Error, ErrorCode};
use crate::types::{
    RequestId, 
    Response,
    Json
};

/// A trait for converting any return type into MCP response
pub trait IntoResponse {
    /// Converts a type into MCP server response
    fn into_response(self, req_id: RequestId) -> Response;
}

impl IntoResponse for Response {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        self.set_id(req_id)
    }
}

impl IntoResponse for &'static str {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        let result = serde_json::json!({ "result": self });
        Response::success(req_id, result)
    }
}

impl IntoResponse for Error {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        Response::error(req_id, self)
    }
}

impl IntoResponse for ErrorCode {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        Response::error(req_id, self.into())
    }
}

impl<T: Serialize> IntoResponse for Json<T> {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        match serde_json::to_value(self) {
            Ok(v) => Response::success(req_id, v),
            Err(err) => Response::error(req_id, err.into())
        }
    }
}

impl IntoResponse for serde_json::Value {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        Response::success(req_id, self)
    }
}

impl IntoResponse for () {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        Response::empty(req_id)
    }
}

impl<T, E> IntoResponse for Result<T, E>
where 
    T: IntoResponse,
    E: IntoResponse
{
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        match self { 
            Ok(value) => value.into_response(req_id),
            Err(err) => err.into_response(req_id),
        }
    }
}

macro_rules! impl_into_response {
    { $($type:ident),* $(,)? } => {
        $(impl IntoResponse for $type {
            #[inline]
            fn into_response(self, req_id: RequestId) -> Response {
                let result = serde_json::json!({ "result": self });
                Response::success(req_id, result)
            }
        })*
    };
}

impl_into_response! {
    String, bool,
    i8, i16, i32, i64, i128, isize,
    u8, u16, u32, u64, u128, usize,
    f32, f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn it_converts_str_into_response() {
        let resp = "test".into_response(RequestId::default());

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"result":"test"}}"#);
    }

    #[test]
    fn it_converts_string_into_response() {
        let resp = String::from("test").into_response(RequestId::default());

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"result":"test"}}"#);
    }

    #[test]
    fn it_converts_i8_into_response() {
        let resp = 1i8.into_response(RequestId::default());

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"result":1}}"#);
    }
    #[test]
    fn it_converts_i16_into_response() {
        let resp = 1i16.into_response(RequestId::default());

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"result":1}}"#);
    }

    #[test]
    fn it_converts_i32_into_response() {
        let resp = 1i32.into_response(RequestId::default());

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"result":1}}"#);
    }

    #[test]
    fn it_converts_i64_into_response() {
        let resp = 1i64.into_response(RequestId::default());

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"result":1}}"#);
    }

    #[test]
    fn it_converts_i128_into_response() {
        let resp = 1i128.into_response(RequestId::default());

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"result":1}}"#);
    }

    #[test]
    fn it_converts_isize_into_response() {
        let resp = 1isize.into_response(RequestId::default());

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"result":1}}"#);
    }

    #[test]
    fn it_converts_u8_into_response() {
        let resp = 1u8.into_response(RequestId::default());

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"result":1}}"#);
    }

    #[test]
    fn it_converts_u16_into_response() {
        let resp = 1u16.into_response(RequestId::default());

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"result":1}}"#);
    }

    #[test]
    fn it_converts_u32_into_response() {
        let resp = 1u32.into_response(RequestId::default());

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"result":1}}"#);
    }

    #[test]
    fn it_converts_u64_into_response() {
        let resp = 1u64.into_response(RequestId::default());

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"result":1}}"#);
    }

    #[test]
    fn it_converts_u128_into_response() {
        let resp = 1u128.into_response(RequestId::default());

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"result":1}}"#);
    }

    #[test]
    fn it_converts_usize_into_response() {
        let resp = 1usize.into_response(RequestId::default());

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"result":1}}"#);
    }

    #[test]
    fn it_converts_f32_into_response() {
        let resp = 1.5f32.into_response(RequestId::default());

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"result":1.5}}"#);
    }

    #[test]
    fn it_converts_f64_into_response() {
        let resp = 1.5f64.into_response(RequestId::default());

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"result":1.5}}"#);
    }
    
    #[test]
    fn it_converts_bool_into_response() {
        let resp = true.into_response(RequestId::default());

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"result":true}}"#);
    }

    #[test]
    fn it_converts_json_into_response() {
        let json = Json::from(Test { name: "test".into() });
        let resp = json.into_response(RequestId::default());

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"name":"test"}}"#);
    }

    #[test]
    fn it_converts_untyped_json_into_response() {
        let json = serde_json::json!({ "some": "prop" });
        let resp = json.into_response(RequestId::default());

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"some":"prop"}}"#);
    }
    
    #[derive(Serialize)]
    struct Test {
        name: String
    }
}

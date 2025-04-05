//! Types and util for handling tool results

use serde::Serialize;
use crate::error::Error;
use crate::types::{Content, IntoResponse, Json, RequestId, Response};

/// The server's response to a tool call.
///
/// Any errors that originate from the tool SHOULD be reported inside the result
/// object, with `isError` set to true, _not_ as an MCP protocol-level error
/// response. Otherwise, the LLM would not be able to see that an error occurred
/// and self-correct.
///
/// However, any errors in _finding_ the tool, an error indicating that the
/// server does not support tool calls, or any other exceptional conditions,
/// should be reported as an MCP error response.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Serialize)]
pub struct CallToolResponse {
    /// The server's response to a tools/call request from the client.
    pub content: Vec<Content>,

    /// Whether the tool call was unsuccessful. If true, the call was unsuccessful.
    pub is_error: bool,
}

impl IntoResponse for CallToolResponse {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        Response::success(req_id, serde_json::to_value(self).unwrap())
    }
}

impl From<Error> for CallToolResponse {
    #[inline]
    fn from(value: Error) -> Self {
        Self::error(value)
    }
}

impl<T, E> From<Result<T, E>> for CallToolResponse
where
    T: Into<CallToolResponse>,
    E: Into<Error>,
{
    #[inline]
    fn from(value: Result<T, E>) -> Self {
        match value { 
            Ok(value) => value.into(),
            Err(error) => error.into().into(),
        }
    }
}

impl<T> From<Option<T>> for CallToolResponse
where
    T: Into<CallToolResponse>,
{
    #[inline]
    fn from(value: Option<T>) -> Self {
        match value {
            Some(value) => value.into(),
            None => Self::empty(),
        }
    }
}

impl From<&'static str> for CallToolResponse {
    #[inline]
    fn from(str: &str) -> Self {
        Self::text(str)
    }
}

impl From<String> for CallToolResponse {
    #[inline]
    fn from(str: String) -> Self {
        Self::text(&str)
    }
}

impl<T: Serialize> From<Json<T>> for CallToolResponse {
    #[inline]
    fn from(value: Json<T>) -> Self {
        serde_json::to_value(&value)
            .map_err(Error::from)
            .into()
    }
}

impl From<Vec<&'static str>> for CallToolResponse {
    #[inline]
    fn from(values: Vec<&'static str>) -> Self {
        Self::texts(values)
    }
}

impl From<serde_json::Value> for CallToolResponse {
    #[inline]
    fn from(value: serde_json::Value) -> Self {
        value.to_string().into()
    }
}

macro_rules! impl_from_for_call_tool_response {
    { $($type:ident),* $(,)? } => {
        $(impl From<$type> for CallToolResponse {
            #[inline]
            fn from(value: $type) -> Self {
                Self::text(&value.to_string())
            }
        })*
    };
}

impl_from_for_call_tool_response! {
    bool,
    i8, i16, i32, i64, i128, isize,
    u8, u16, u32, u64, u128, usize,
    f32, f64,
}

impl CallToolResponse {
    /// Creates a single text response
    #[inline]
    pub fn text(text: &str) -> Self {
        Self { 
            content: vec![Content::text(text)],
            is_error: false,
        }
    }

    /// Creates a list of strings response
    #[inline]
    pub fn texts<T>(texts: T) -> Self
    where
        T: IntoIterator<Item = &'static str>
    {
        let content = texts
            .into_iter()
            .map(Content::text)
            .collect();
        Self { content, is_error: false }
    }

    /// Creates an error response
    #[inline]
    pub fn error(error: Error) -> Self {
        Self {
            content: vec![Content::text(&error.to_string())],
            is_error: true,
        }
    }

    /// Creates an empty response
    #[inline]
    pub fn empty() -> Self {
        Self {
            content: vec![],
            is_error: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::error::ErrorCode;

    use super::*;
    
    #[test]
    fn it_converts_from_str() {
        let resp: CallToolResponse = "test".into();
        
        let json = serde_json::to_string(&resp).unwrap();
        
        assert_eq!(json, r#"{"content":[{"type":"text","text":"test","mimeType":"text/plain"}],"is_error":false}"#);
    }

    #[test]
    fn it_converts_from_string() {
        let resp: CallToolResponse = String::from("test").into();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[{"type":"text","text":"test","mimeType":"text/plain"}],"is_error":false}"#);
    }

    #[test]
    fn it_converts_from_error() {
        let resp: CallToolResponse = Error::new(ErrorCode::InternalError, "test").into();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[{"type":"text","text":"test","mimeType":"text/plain"}],"is_error":true}"#);
    }

    #[test]
    fn it_converts_from_err_result() {
        let resp: CallToolResponse = Err::<String, _>(Error::new(ErrorCode::InternalError, "test")).into();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[{"type":"text","text":"test","mimeType":"text/plain"}],"is_error":true}"#);
    }

    #[test]
    fn it_converts_from_ok_result() {
        let resp: CallToolResponse = Ok::<_, Error>("test").into();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[{"type":"text","text":"test","mimeType":"text/plain"}],"is_error":false}"#);
    }

    #[test]
    fn it_converts_from_some_option_result() {
        let resp: CallToolResponse = Some("test").into();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[{"type":"text","text":"test","mimeType":"text/plain"}],"is_error":false}"#);
    }

    #[test]
    fn it_converts_from_none_option_result() {
        let resp: CallToolResponse = None::<String>.into();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[],"is_error":false}"#);
    }

    #[test]
    fn it_converts_from_vec() {
        let resp: CallToolResponse = vec!["test 1", "test 2"].into();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[{"type":"text","text":"test 1","mimeType":"text/plain"},{"type":"text","text":"test 2","mimeType":"text/plain"}],"is_error":false}"#);
    }

    #[test]
    #[allow(clippy::useless_conversion)]
    fn it_converts_from_self() {
        let resp: CallToolResponse = CallToolResponse::empty();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[],"is_error":false}"#);
    }

    #[test]
    fn it_converts_from_untyped_json() {
        let resp: CallToolResponse = serde_json::json!({ "msg": "test" }).into();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[{"type":"text","text":"{\"msg\":\"test\"}","mimeType":"text/plain"}],"is_error":false}"#);
    }

    #[test]
    fn it_converts_from_typed_json() {
        let json = Test { msg: "test".into() };
        let resp: CallToolResponse = Json::from(json).into();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[{"type":"text","text":"{\"msg\":\"test\"}","mimeType":"text/plain"}],"is_error":false}"#);
    }
    
    #[derive(Serialize)]
    struct Test {
        msg: String
    }
}
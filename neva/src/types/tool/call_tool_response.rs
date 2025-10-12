//! Types and util for handling tool results

use crate::types::{Content, IntoResponse, RequestId, Response};
use serde::{Serialize, Deserialize};
#[cfg(feature = "server")]
use crate::types::Json;
#[cfg(any(feature = "server", feature = "client"))]
use {
    crate::error::Error,
    serde_json::Value,
};
#[cfg(feature = "client")]
use {
    crate::error::ErrorCode,
    serde::de::DeserializeOwned
};

#[cfg(feature = "client")]
const MISSING_STRUCTURED_CONTENT: &str = "Tool: Missing structured content";

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
#[derive(Serialize, Deserialize)]
pub struct CallToolResponse {
    /// The server's response to a tools/call request from the client.
    pub content: Vec<Content>,
    
    /// An optional JSON object that represents the structured result of the tool call.
    #[serde(rename = "structuredContent", skip_serializing_if = "Option::is_none")]
    pub struct_content: Option<serde_json::Value>,

    /// Whether the tool call was unsuccessful. If true, the call was unsuccessful.
    #[serde(default, rename = "isError")]
    pub is_error: bool,
}

impl IntoResponse for CallToolResponse {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        Response::success(req_id, serde_json::to_value(self).unwrap())
    }
}

#[cfg(feature = "server")]
impl From<Error> for CallToolResponse {
    #[inline]
    fn from(value: Error) -> Self {
        Self::error(value)
    }
}

#[cfg(feature = "server")]
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

#[cfg(feature = "server")]
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

#[cfg(feature = "server")]
impl From<()> for CallToolResponse {
    #[inline]
    fn from(_: ()) -> Self {
        Self::empty()
    }
}

#[cfg(feature = "server")]
impl From<&'static str> for CallToolResponse {
    #[inline]
    fn from(str: &str) -> Self {
        Self::new(str)
    }
}

#[cfg(feature = "server")]
impl From<String> for CallToolResponse {
    #[inline]
    fn from(str: String) -> Self {
        Self::new(str)
    }
}

#[cfg(feature = "server")]
impl From<Content> for CallToolResponse {
    #[inline]
    fn from(content: Content) -> Self {
        Self::new(content)
    }
}

#[cfg(feature = "server")]
impl<T: Serialize> From<Json<T>> for CallToolResponse {
    #[inline]
    fn from(value: Json<T>) -> Self {
        Self::json(value.0)
    }
}

#[cfg(feature = "server")]
impl From<Vec<&'static str>> for CallToolResponse {
    #[inline]
    fn from(values: Vec<&'static str>) -> Self {
        Self::array(values)
    }
}

#[cfg(feature = "server")]
impl From<serde_json::Value> for CallToolResponse {
    #[inline]
    fn from(value: serde_json::Value) -> Self {
        value.to_string().into()
    }
}

#[cfg(feature = "server")]
macro_rules! impl_from_for_call_tool_response {
    { $($type:ident),* $(,)? } => {
        $(impl From<$type> for CallToolResponse {
            #[inline]
            fn from(value: $type) -> Self {
                Self::new(value.to_string())
            }
        })*
    };
}

#[cfg(feature = "server")]
impl_from_for_call_tool_response! {
    bool,
    i8, i16, i32, i64, i128, isize,
    u8, u16, u32, u64, u128, usize,
    f32, f64,
}

#[cfg(feature = "server")]
impl CallToolResponse {
    /// Creates a single response
    #[inline]
    pub fn new(text: impl Into<Content>) -> Self {
        Self { 
            content: vec![text.into()],
            struct_content: None,
            is_error: false,
        }
    }

    /// Creates an array of strings response
    #[inline]
    pub fn array<T, I>(texts: T) -> Self
    where
        T: IntoIterator<Item = I>,
        I: Into<Content>
    {
        let content = texts
            .into_iter()
            .map(Into::into)
            .collect();
        Self { content, struct_content: None, is_error: false }
    }
    
    /// Creates a single structured JSON response
    #[inline]
    pub fn json<T: Serialize>(data: T) -> Self {
        match serde_json::to_value(&data) {
            Err(err) => Self::error(err.into()),
            Ok(structure) => Self {
                content: vec![Content::json(&data)],
                struct_content: Some(structure),
                is_error: false,
            },
        }
    }

    /// Creates an array of structured JSON data response
    #[inline]
    pub fn array_json<T, I>(data: T) -> Self
    where
        T: IntoIterator<Item = I>,
        I: Serialize
    {
        let vec = data.into_iter().collect::<Vec<I>>();
        match serde_json::to_value(&vec) { 
            Err(err) => Self::error(err.into()),
            Ok(structure) => Self {
                struct_content: Some(structure),
                is_error: false,
                content: vec.into_iter()
                    .map(|item| Content::json(&item))
                    .collect::<Vec<Content>>(),
            }
        }
    }

    /// Creates an error response
    #[inline]
    pub fn error(error: Error) -> Self {
        Self {
            content: vec![Content::text(error.to_string())],
            struct_content: None,
            is_error: true,
        }
    }

    /// Creates an empty response
    #[inline]
    pub fn empty() -> Self {
        Self {
            content: vec![],
            struct_content: None,
            is_error: false,
        }
    }

    /// Creates a structure for existing text content.
    /// 
    /// **Note:** If the content type is not a text, this won't get any effect.
    #[inline]
    pub fn with_structure(mut self) -> Self {
        let item = &self.content[0];
        if self.content.len() == 1 {
            if let Content::Text(text) = item {
                match serde_json::from_str(&text.text) { 
                    Ok(structure) => self.struct_content = Some(structure),
                    Err(err) => return Self::error(err.into()),
                }
            }
        } else if let Content::Text(_) = item {
            let data = self.content
                .iter()
                .filter_map(|item| item
                    .as_text()
                    .and_then(|c| serde_json::from_str(&c.text).ok()))
                .collect::<Vec<Value>>();
            match serde_json::to_value(&data) {
                Ok(structure) => self.struct_content = Some(structure),
                Err(err) => return Self::error(err.into()),
            }
        }
        self
    }
}

#[cfg(feature = "client")]
impl CallToolResponse {
    /// Turns [`CallToolResponse`]'s structured content into `T`
    pub fn as_json<T: DeserializeOwned>(&self) -> Result<T, Error> {
        self.struct_content()
            .and_then(|c| serde_json::from_value(c.clone()).map_err(Into::into))
    }
    
    /// Returns a reference to a [`Value`] of structured content
    pub(crate) fn struct_content(&self) -> Result<&Value, Error> {
        self.struct_content
            .as_ref()
            .ok_or_else(|| Error::new(ErrorCode::ParseError, MISSING_STRUCTURED_CONTENT))
    }
}

#[cfg(test)]
#[cfg(feature = "server")]
mod tests {
    use crate::error::{Error, ErrorCode};

    use super::*;
    
    #[test]
    fn it_converts_from_str() {
        let resp: CallToolResponse = "test".into();
        
        let json = serde_json::to_string(&resp).unwrap();
        
        assert_eq!(json, r#"{"content":[{"type":"text","text":"test"}],"isError":false}"#);
    }

    #[test]
    fn it_converts_from_string() {
        let resp: CallToolResponse = String::from("test").into();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[{"type":"text","text":"test"}],"isError":false}"#);
    }

    #[test]
    fn it_converts_from_error() {
        let resp: CallToolResponse = Error::new(ErrorCode::InternalError, "test").into();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[{"type":"text","text":"test"}],"isError":true}"#);
    }

    #[test]
    fn it_converts_from_err_result() {
        let resp: CallToolResponse = Err::<String, _>(Error::new(ErrorCode::InternalError, "test")).into();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[{"type":"text","text":"test"}],"isError":true}"#);
    }

    #[test]
    fn it_converts_from_ok_result() {
        let resp: CallToolResponse = Ok::<_, Error>("test").into();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[{"type":"text","text":"test"}],"isError":false}"#);
    }

    #[test]
    fn it_converts_from_some_option_result() {
        let resp: CallToolResponse = Some("test").into();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[{"type":"text","text":"test"}],"isError":false}"#);
    }

    #[test]
    fn it_converts_from_none_option_result() {
        let resp: CallToolResponse = None::<String>.into();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[],"isError":false}"#);
    }

    #[test]
    fn it_converts_from_vec() {
        let resp: CallToolResponse = vec!["test 1", "test 2"].into();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[{"type":"text","text":"test 1"},{"type":"text","text":"test 2"}],"isError":false}"#);
    }

    #[test]
    #[allow(clippy::useless_conversion)]
    fn it_converts_from_self() {
        let resp: CallToolResponse = CallToolResponse::empty();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[],"isError":false}"#);
    }

    #[test]
    fn it_converts_from_untyped_json() {
        let resp: CallToolResponse = serde_json::json!({ "msg": "test" }).into();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[{"type":"text","text":"{\"msg\":\"test\"}"}],"isError":false}"#);
    }

    #[test]
    fn it_converts_from_typed_json() {
        let json = Test { msg: "test".into() };
        let resp: CallToolResponse = Json::from(json).into();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[{"type":"text","text":"{\"msg\":\"test\"}"}],"structuredContent":{"msg":"test"},"isError":false}"#);
    }

    #[test]
    fn it_creates_with_structured_content() {
        let json = Test { msg: "test".into() };
        let resp = CallToolResponse::json(json);

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[{"type":"text","text":"{\"msg\":\"test\"}"}],"structuredContent":{"msg":"test"},"isError":false}"#);
    }

    #[test]
    fn it_creates_with_array_of_structured_content() {
        let resp = CallToolResponse::array_json([
            Test { msg: "test 1".into() },
            Test { msg: "test 2".into() }
        ]);

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[{"type":"text","text":"{\"msg\":\"test 1\"}"},{"type":"text","text":"{\"msg\":\"test 2\"}"}],"structuredContent":[{"msg":"test 1"},{"msg":"test 2"}],"isError":false}"#);
    }

    #[test]
    fn it_adds_structured_content() {
        let resp = CallToolResponse::new(r#"{"msg":"test"}"#)
            .with_structure();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[{"type":"text","text":"{\"msg\":\"test\"}"}],"structuredContent":{"msg":"test"},"isError":false}"#);
    }

    #[test]
    fn it_adds_structured_content_for_string_array() {
        let resp = CallToolResponse::new(r#"[{"msg":"test 1"},{"msg":"test 2"}]"#)
            .with_structure();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[{"type":"text","text":"[{\"msg\":\"test 1\"},{\"msg\":\"test 2\"}]"}],"structuredContent":[{"msg":"test 1"},{"msg":"test 2"}],"isError":false}"#);
    }

    #[test]
    fn it_adds_structured_content_for_array() {
        let resp = CallToolResponse::array([
            r#"{"msg":"test 1"}"#,
            r#"{"msg":"test 2"}"#
        ]).with_structure();

        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"content":[{"type":"text","text":"{\"msg\":\"test 1\"}"},{"type":"text","text":"{\"msg\":\"test 2\"}"}],"structuredContent":[{"msg":"test 1"},{"msg":"test 2"}],"isError":false}"#);
    }
    
    #[derive(Serialize)]
    struct Test {
        msg: String
    }
}
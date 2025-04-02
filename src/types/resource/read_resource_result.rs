//! Types and utils for handling read resource results

use base64::{engine::general_purpose, Engine};
use serde::{Deserialize, Serialize};
use crate::types::{IntoResponse, RequestId, Response};

/// The server's response to a resources/read request from the client.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Serialize)]
pub struct ReadResourceResult {
    /// A list of ResourceContents that this resource contains.
    pub contents: Vec<ResourceContents>
}

/// Represents the content of a resource.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Serialize, Deserialize)]
pub struct ResourceContents {
    /// The URI of the resource.
    pub uri: String,

    /// The type of content.
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,

    /// The text content of the resource.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    /// The base64-encoded binary content of the resource.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>
}

impl IntoResponse for ReadResourceResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        Response::success(req_id, serde_json::to_value(self).unwrap())
    }
}

impl From<(&str, &str)> for ResourceContents {
    #[inline]
    fn from((uri, text): (&str, &str)) -> Self {
        Self {
            uri: uri.into(),
            text: Some(text.into()),
            mime: Some("text/plain".into()),
            blob: None
        }
    }
}

impl From<(&str, &str, &str)> for ResourceContents {
    #[inline]
    fn from((uri, mime, text): (&str, &str, &str)) -> Self {
        Self {
            uri: uri.into(),
            text: Some(text.into()),
            mime: Some(mime.into()),
            blob: None
        }
    }
}

impl From<(String, String)> for ResourceContents {
    #[inline]
    fn from((uri, text): (String, String)) -> Self {
        Self {
            uri,
            text: Some(text),
            mime: Some("text/plain".into()),
            blob: None
        }
    }
}

impl From<(String, String, String)> for ResourceContents {
    #[inline]
    fn from((uri, mime, text): (String, String, String)) -> Self {
        Self {
            uri,
            text: Some(text),
            mime: Some(mime),
            blob: None
        }
    }
}

impl From<ResourceContents> for ReadResourceResult {
    #[inline]
    fn from(content: ResourceContents) -> Self {
        Self { contents: vec![content] }
    }
}

impl<T, R> From<T> for ReadResourceResult
where 
    T: IntoIterator<Item = R>,
    R: Into<ResourceContents>
{
    #[inline]
    fn from(iter: T) -> Self {
        Self {
            contents: iter
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

impl ReadResourceResult {
    /// Creates a text resource result
    #[inline]
    pub fn text(uri: &str, mime: &str, text: &str) -> Self {
        Self {
            contents: vec![ResourceContents::text(uri, mime, text)]
        }
    }

    /// Creates a blob resource result
    #[inline]
    pub fn blob(uri: &str, mime: &str, blob: impl AsRef<[u8]>) -> Self {
        Self {
            contents: vec![ResourceContents::blob(uri, mime, blob)]
        }
    }
}

impl ResourceContents {
    /// Creates a text resource content
    #[inline]
    pub fn text(uri: &str, mime: &str, text: &str) -> Self {
        Self {
            uri: uri.into(),
            mime: Some(mime.into()),
            text: Some(text.into()),
            blob: None
        }
    }

    /// Creates a blob resource content
    #[inline]
    pub fn blob(uri: &str, mime: &str, blob: impl AsRef<[u8]>) -> ResourceContents {
        let blob = general_purpose::STANDARD.encode(blob);
        Self {
            uri: uri.into(),
            mime: Some(mime.into()),
            blob: Some(blob),
            text: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn it_creates_result_from_array_of_contents() {
        let result = ReadResourceResult::from([
            ResourceContents::text("/res1", "plain/text", "test 1"),
            ResourceContents::text("/res1", "plain/text", "test 1")
        ]);

        let json = serde_json::to_string(&result).unwrap();

        assert_eq!(json, r#"{"contents":[{"uri":"/res1","mimeType":"plain/text","text":"test 1"},{"uri":"/res1","mimeType":"plain/text","text":"test 1"}]}"#);
    }

    #[test]
    fn it_creates_result_from_array_of_str_tuples1() {
        let result = ReadResourceResult::from([
            ("/res1", "test 1"),
            ("/res1", "test 1")
        ]);

        let json = serde_json::to_string(&result).unwrap();

        assert_eq!(json, r#"{"contents":[{"uri":"/res1","mimeType":"text/plain","text":"test 1"},{"uri":"/res1","mimeType":"text/plain","text":"test 1"}]}"#);
    }

    #[test]
    fn it_creates_result_from_array_of_str_tuples2() {
        let result = ReadResourceResult::from([
            ("/res1", "json", "test 1"),
            ("/res1", "json", "test 1")
        ]);

        let json = serde_json::to_string(&result).unwrap();

        assert_eq!(json, r#"{"contents":[{"uri":"/res1","mimeType":"json","text":"test 1"},{"uri":"/res1","mimeType":"json","text":"test 1"}]}"#);
    }

    #[test]
    fn it_creates_result_from_array_of_string_tuples1() {
        let result = ReadResourceResult::from([
            (String::from("/res1"), String::from("test 1")),
            (String::from("/res1"), String::from("test 1"))
        ]);

        let json = serde_json::to_string(&result).unwrap();

        assert_eq!(json, r#"{"contents":[{"uri":"/res1","mimeType":"text/plain","text":"test 1"},{"uri":"/res1","mimeType":"text/plain","text":"test 1"}]}"#);
    }

    #[test]
    fn it_creates_result_from_array_of_string_tuples2() {
        let result = ReadResourceResult::from([
            (String::from("/res1"), String::from("json"), String::from("test 1")),
            (String::from("/res1"), String::from("json"), String::from("test 1"))
        ]);

        let json = serde_json::to_string(&result).unwrap();

        assert_eq!(json, r#"{"contents":[{"uri":"/res1","mimeType":"json","text":"test 1"},{"uri":"/res1","mimeType":"json","text":"test 1"}]}"#);
    }

    #[test]
    fn it_creates_result_from_tuple_of_contents() {
        let content: ResourceContents = ("/res", "test").into();
        let result: ReadResourceResult = content.into();

        let json = serde_json::to_string(&result).unwrap();

        assert_eq!(json, r#"{"contents":[{"uri":"/res","mimeType":"text/plain","text":"test"}]}"#);
    }
}
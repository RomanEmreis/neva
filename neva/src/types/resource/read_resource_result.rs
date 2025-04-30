//! Types and utils for handling read resource results

use serde::{Deserialize, Serialize};
#[cfg(feature = "server")]
use base64::{engine::general_purpose, Engine};
#[cfg(feature = "server")]
use crate::{error::Error, types::{IntoResponse, RequestId, Response}};

/// The server's response to a resources/read request from the client.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Serialize, Deserialize)]
pub struct ReadResourceResult {
    /// A list of ResourceContents that this resource contains.
    pub contents: Vec<ResourceContents>
}

/// Represents the content of a resource.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Serialize, Deserialize)]
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

#[cfg(feature = "server")]
impl IntoResponse for ReadResourceResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        Response::success(req_id, serde_json::to_value(self).unwrap())
    }
}

#[cfg(feature = "server")]
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

#[cfg(feature = "server")]
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

#[cfg(feature = "server")]
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

#[cfg(feature = "server")]
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

#[cfg(feature = "server")]
impl<T> From<T> for ReadResourceResult
where 
    T: Into<ResourceContents>
{
    fn from(content: T) -> Self {
        Self { contents: vec![content.into()] }
    }
}

#[cfg(feature = "server")]
impl<T, E> TryFrom<Result<T, E>> for ReadResourceResult
where 
    T: Into<ReadResourceResult>,
    E: Into<Error>
{
    type Error = E;

    #[inline]
    fn try_from(value: Result<T, E>) -> Result<Self, Self::Error> {
        match value {
            Ok(ok) => Ok(ok.into()),
            Err(err) => Err(err)
        }
    }
}

#[cfg(feature = "server")]
impl<T> From<Vec<T>> for ReadResourceResult
where 
    T: Into<ResourceContents>
{
    #[inline]
    fn from(vec: Vec<T>) -> Self {
        Self {
            contents: vec
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

#[cfg(feature = "server")]
impl<const N: usize, T> From<[T; N]> for ReadResourceResult
where 
    T: Into<ResourceContents>
{
    #[inline]
    fn from(vec: [T; N]) -> Self {
        Self {
            contents: vec
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

#[cfg(feature = "server")]
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

#[cfg(feature = "server")]
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
#[cfg(feature = "server")]
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
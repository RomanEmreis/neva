//! Types and utils for handling read resource results

use serde::{Deserialize, Serialize};
#[cfg(feature = "server")]
use base64::{engine::general_purpose, Engine};
#[cfg(feature = "server")]
use crate::{error::Error, types::{IntoResponse, RequestId, Response}};
use crate::types::Uri;

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
    pub uri: Uri,

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

impl Default for ReadResourceResult {
    #[inline]
    fn default() -> Self {
        Self {
            contents: Vec::with_capacity(8)
        }
    }
}

#[cfg(feature = "server")]
impl<T1, T2> From<(T1, T2)> for ResourceContents 
where 
    T1: Into<Uri>,
    T2: Into<String>
{
    #[inline]
    fn from((uri, text): (T1, T2)) -> Self {
        Self {
            uri: uri.into(),
            text: Some(text.into()),
            mime: Some("text/plain".into()),
            blob: None
        }
    }
}

#[cfg(feature = "server")]
impl<T1, T2, T3> From<(T1, T2, T3)> for ResourceContents
where
    T1: Into<Uri>,
    T2: Into<String>,
    T3: Into<String>
{
    #[inline]
    fn from((uri, mime, text): (T1, T2, T3)) -> Self {
        Self {
            uri: uri.into(),
            text: Some(text.into()),
            mime: Some(mime.into()),
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
    /// Creates a new read resource result
    #[inline]
    pub fn new() -> Self {
        Self::default()  
    }
    
    /// Add a resource content to the result
    #[inline]
    pub fn with_content(mut self, content: impl Into<ResourceContents>) -> Self {
        self.contents.push(content.into());
        self
    }
    
    /// Add multiple resource contents to the result
    pub fn with_contents<T, I>(mut self, contents: T) -> Self
    where 
        T: IntoIterator<Item = I>,
        I: Into<ResourceContents>
    {
        self.contents
            .extend(contents.into_iter().map(Into::into));
        self
    }
}

#[cfg(feature = "server")]
impl ResourceContents {
    /// Creates a resource content
    #[inline]
    pub  fn new(uri: impl Into<Uri>) -> Self {
        Self {
            uri: uri.into(),
            mime: None,
            text: None,
            blob: None
        }
    }
    
    /// Sets the mime type of the resource content
    #[inline]
    pub fn with_mime(mut self, mime: impl Into<String>) -> Self {
        self.mime = Some(mime.into());
        self
    }
    
    /// Sets the text content of the resource
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self.blob = None;
        self
    }
    
    /// Sets the binary content of the resource
    pub fn with_blob(mut self, blob: impl AsRef<[u8]>) -> Self {
        self.blob = Some(general_purpose::STANDARD.encode(blob));
        self.text = None;
        self
    }
}

#[cfg(test)]
#[cfg(feature = "server")]
mod tests {
    use super::*;
    
    #[test]
    fn it_creates_result_from_array_of_contents() {
        let result = ReadResourceResult::from([
            ResourceContents::new("/res1")
                .with_mime("plain/text")
                .with_text("test 1"),
            ResourceContents::new("/res1")
                .with_mime("plain/text")
                .with_text("test 1")
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
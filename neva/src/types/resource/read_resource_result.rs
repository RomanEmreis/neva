//! Types and utils for handling read resource results

use serde::{Deserialize, Serialize};
use bytes::Bytes;
use crate::types::{Annotations, Uri};
use crate::types::helpers::{deserialize_base64_as_bytes, serialize_bytes_as_base64};
#[cfg(feature = "server")]
use {
    crate::{error::{Error, ErrorCode}, types::{IntoResponse, RequestId, Response}},
    serde::de::DeserializeOwned
};

const CHUNK_SIZE: usize = 8192;

/// The server's response to a resources/read request from the client.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Serialize, Deserialize)]
pub struct ReadResourceResult {
    /// A list of ResourceContents that this resource contains.
    pub contents: Vec<ResourceContents>
}

/// Represents the content of a resource.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResourceContents {
    /// Represents a text resource content
    Text(TextResourceContents),
    
    /// Represents a JSON resource content
    Json(JsonResourceContents),
    
    /// Represents a blob resource content
    Blob(BlobResourceContents),
    
    /// Represents an empty/unknown resource content
    Empty(EmptyResourceContents),
}

/// Represents a blob resource content
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobResourceContents {
    /// The URI of the resource.
    pub uri: Uri,

    /// Raw binary data of the resource.
    ///
    /// **Note:** will be serialized as a base64-encoded string
    #[serde(
        serialize_with = "serialize_bytes_as_base64",
        deserialize_with = "deserialize_base64_as_bytes")]
    pub blob: Bytes,

    /// Intended for UI and end-user contexts - optimized to be human-readable and easily understood,
    /// even by those unfamiliar with domain-specific terminology.
    ///
    /// If not provided, the name should be used for display (except for Tool,
    /// where `annotations.title` should be given precedence over using `name`, if present).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// The MIME type of content.
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,

    /// Optional annotations for the client.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,

    /// Metadata reserved by MCP for protocol-level metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

/// Represents a text resource content
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextResourceContents {
    /// The URI of the resource.
    pub uri: Uri,

    /// The text content of the resource.
    pub text: String,

    /// Intended for UI and end-user contexts - optimized to be human-readable and easily understood,
    /// even by those unfamiliar with domain-specific terminology.
    ///
    /// If not provided, the name should be used for display (except for Tool,
    /// where `annotations.title` should be given precedence over using `name`, if present).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// The MIME type of content.
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,

    /// Optional annotations for the client.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,

    /// Metadata reserved by MCP for protocol-level metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

/// Represents a JSON resource content
/// 
/// > **Note:** This is a specialization of [`TextResourceContents`] for JSON content.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonResourceContents {
    /// The URI of the resource.
    pub uri: Uri,

    /// The JSON content of the resource.
    #[serde(rename = "text")]
    pub value: serde_json::Value,

    /// Intended for UI and end-user contexts - optimized to be human-readable and easily understood,
    /// even by those unfamiliar with domain-specific terminology.
    ///
    /// If not provided, the name should be used for display (except for Tool,
    /// where `annotations.title` should be given precedence over using `name`, if present).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// The MIME type of content.
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,

    /// Optional annotations for the client.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,

    /// Metadata reserved by MCP for protocol-level metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

/// Represents an empty/unknown resource content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmptyResourceContents {
    /// The URI of the resource.
    pub uri: Uri,

    /// Intended for UI and end-user contexts - optimized to be human-readable and easily understood,
    /// even by those unfamiliar with domain-specific terminology.
    ///
    /// If not provided, the name should be used for display (except for Tool,
    /// where `annotations.title` should be given precedence over using `name`, if present).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// The MIME type of content.
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,

    /// Optional annotations for the client.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,

    /// Metadata reserved by MCP for protocol-level metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
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

impl From<TextResourceContents> for ResourceContents {
    #[inline]
    fn from(value: TextResourceContents) -> Self {
        Self::Text(value)
    }
}

impl From<JsonResourceContents> for ResourceContents {
    #[inline]
    fn from(value: JsonResourceContents) -> Self {
        Self::Json(value)
    }
}

impl From<BlobResourceContents> for ResourceContents {
    #[inline]
    fn from(value: BlobResourceContents) -> Self {
        Self::Blob(value)
    }
}

#[cfg(feature = "server")]
impl<T1, T2> From<(T1, T2)> for TextResourceContents 
where 
    T1: Into<Uri>,
    T2: Into<String>
{
    #[inline]
    fn from((uri, text): (T1, T2)) -> Self {
        Self {
            uri: uri.into(),
            text: text.into(),
            mime: Some("text/plain".into()),
            title: None,
            annotations: None,
            meta: None,
        }
    }
}

#[cfg(feature = "server")]
impl<T1, T2, T3> From<(T1, T2, T3)> for TextResourceContents
where
    T1: Into<Uri>,
    T2: Into<String>,
    T3: Into<String>
{
    #[inline]
    fn from((uri, mime, text): (T1, T2, T3)) -> Self {
        Self {
            uri: uri.into(),
            text: text.into(),
            mime: Some(mime.into()),
            title: None,
            annotations: None,
            meta: None,
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
    fn from(pair: (T1, T2)) -> Self {
        Self::Text(TextResourceContents::from(pair))
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
    fn from(triplet: (T1, T2, T3)) -> Self {
        Self::Text(TextResourceContents::from(triplet))
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
    /// Creates a new resource content
    #[inline]
    pub fn new(uri: impl Into<Uri>) -> Self {
        Self::Empty(EmptyResourceContents::new(uri))
    }
    
    /// Returns the URI of the resource content
    #[inline]
    pub fn uri(&self) -> &Uri {
        match self {
            Self::Text(text) => &text.uri,
            Self::Json(json) => &json.uri,
            Self::Blob(blob) => &blob.uri,
            Self::Empty(empty) => &empty.uri
        }
    }

    /// Returns the URI of the resource content
    #[inline]
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(&text.text),
            Self::Json(json) => json.value.as_str(),
            Self::Blob(_) => None,
            Self::Empty(_) => None
        }
    }

    /// Returns the title of resource
    #[inline]
    pub fn title(&self) -> Option<&str> {
        match self {
            Self::Text(text) => text.title.as_deref(),
            Self::Json(json) => json.title.as_deref(),
            Self::Blob(blob) => blob.title.as_deref(),
            Self::Empty(empty) => empty.title.as_deref()
        }
    }

    /// Returns the annotations of resource
    #[inline]
    pub fn annotations(&self) -> Option<&Annotations> {
        match self {
            Self::Text(text) => text.annotations.as_ref(),
            Self::Json(json) => json.annotations.as_ref(),
            Self::Blob(blob) => blob.annotations.as_ref(),
            Self::Empty(empty) => empty.annotations.as_ref()
        }
    }

    /// Returns the URI of the resource content
    #[inline]
    pub fn blob(&self) -> Option<&[u8]> {
        match self {
            Self::Blob(blob) => Some(&blob.blob),
            Self::Json(_) => None,
            Self::Text(_) => None,
            Self::Empty(_) => None
        }
    }

    /// Returns the URI of the resource content
    #[inline]
    pub fn json<T: DeserializeOwned>(&self) -> Result<T, Error> {
        match self {
            Self::Text(text) => serde_json::from_str(&text.text)
                .map_err(Error::from),
            Self::Json(json) => serde_json::from_value(json.value.clone())
                .map_err(Error::from),
            Self::Blob(_) => Err(Error::new(ErrorCode::InvalidRequest, "Cannot deserialize blob")),
            Self::Empty(_) => Err(Error::new(ErrorCode::InvalidRequest, "Cannot empty resource"))
        }
    }

    /// Returns the mime type of the resource content
    #[inline]
    pub fn mime(&self) -> Option<&str> {
        match self { 
            Self::Text(text) => text.mime.as_deref(),
            Self::Json(json) => json.mime.as_deref(),
            Self::Blob(blob) => blob.mime.as_deref(),
            Self::Empty(empty) => empty.mime.as_deref()
        }
    }
    
    /// Sets the mime type of the resource content
    #[inline]
    pub fn with_mime(mut self, mime: impl Into<String>) -> Self {
        match self {
            Self::Text(ref mut text) => text.mime = Some(mime.into()),
            Self::Json(ref mut json) => json.mime = Some(mime.into()),
            Self::Blob(ref mut blob) => blob.mime = Some(mime.into()),
            Self::Empty(ref mut empty) => empty.mime = Some(mime.into()),
        }
        self
    }

    /// Sets the title of the resource
    #[inline]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        match self {
            Self::Text(ref mut text) => text.title = Some(title.into()),
            Self::Json(ref mut json) => json.title = Some(title.into()),
            Self::Blob(ref mut blob) => blob.title = Some(title.into()),
            Self::Empty(ref mut empty) => empty.title = Some(title.into()),
        }
        self
    }

    /// Sets annotations for the client
    #[inline]
    pub fn with_annotations<F>(self, config: F) -> Self
    where
        F: FnOnce(Annotations) -> Annotations
    {
        match self {
            Self::Text(text) => Self::Text(text.with_annotations(config)),
            Self::Json(json) => Self::Json(json.with_annotations(config)),
            Self::Blob(blob) => Self::Blob(blob.with_annotations(config)),
            Self::Empty(empty) => Self::Empty(empty.with_annotations(config)),
        }
    }
    
    /// Sets the text of the resource content and make it [`TextResourceContents`]
    #[inline]
    pub fn with_text(self, text: impl Into<String>) -> Self {
        let text = text.into();
        match self {
            Self::Text(content) => Self::Text(TextResourceContents {
                uri: content.uri,
                mime: content.mime,
                title: content.title,
                annotations: content.annotations,
                meta: content.meta,
                text,
            }),
            Self::Json(content) => Self::Text(TextResourceContents {
                uri: content.uri,
                mime: Some("text/plain".into()),
                title: content.title,
                annotations: content.annotations,
                meta: content.meta,
                text,
            }),
            Self::Blob(content) => Self::Text(TextResourceContents {
                uri: content.uri,
                mime: Some("text/plain".into()),
                title: content.title,
                annotations: content.annotations,
                meta: content.meta,
                text,
            }),
            Self::Empty(content) => Self::Text(TextResourceContents {
                uri: content.uri,
                mime: content.mime.or_else(|| Some("text/plain".into())),
                title: None,
                annotations: None,
                meta: None,
                text,
            })
        }
    }

    /// Sets the text of the resource content and make it [`TextResourceContents`]
    #[inline]
    pub fn with_blob(self, blob: impl Into<Bytes>) -> Self {
        let blob = blob.into();
        match self {
            Self::Text(content) => Self::Blob(BlobResourceContents {
                uri: content.uri,
                mime: content.mime,
                title: content.title,
                annotations: content.annotations,
                meta: content.meta,
                blob,
            }),
            Self::Json(content) => Self::Blob(BlobResourceContents {
                uri: content.uri,
                mime: content.mime,
                title: content.title,
                annotations: content.annotations,
                meta: content.meta,
                blob,
            }),
            Self::Blob(content) => Self::Blob(BlobResourceContents {
                uri: content.uri,
                mime: None,
                title: content.title,
                annotations: content.annotations,
                meta: content.meta,
                blob,
            }),
            Self::Empty(content) => Self::Blob(BlobResourceContents {
                uri: content.uri,
                mime: None,
                title: None,
                annotations: None,
                meta: None,
                blob,
            })
        }
    }

    /// Sets the JSON text of the resource content and make it [`TextResourceContents`]
    #[inline]
    pub fn with_json<T: Serialize>(self, data: T) -> Self {
        let value = serde_json::to_value(data)
            .expect("Failed to serialize JSON");
        match self {
            Self::Text(content) => Self::Json(JsonResourceContents {
                uri: content.uri,
                mime: Some("application/json".into()),
                title: content.title,
                annotations: content.annotations,
                meta: content.meta,
                value,
            }),
            Self::Json(content) => Self::Json(JsonResourceContents {
                uri: content.uri,
                mime: content.mime.or_else(|| Some("application/json".into())),
                title: content.title,
                annotations: content.annotations,
                meta: content.meta,
                value,
            }),
            Self::Blob(content) => Self::Json(JsonResourceContents {
                uri: content.uri,
                mime: Some("application/json".into()),
                title: content.title,
                annotations: content.annotations,
                meta: content.meta,
                value,
            }),
            Self::Empty(content) => Self::Json(JsonResourceContents {
                uri: content.uri,
                mime: content.mime.or_else(|| Some("application/json".into())),
                title: None,
                annotations: None,
                meta: None,
                value,
            })
        }
    }
}

impl TextResourceContents {
    /// Creates a text resource content
    #[inline]
    pub fn new(uri: impl Into<Uri>, text: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            text: text.into(),
            mime: Some("text/plain".into()),
            title: None,
            annotations: None,
            meta: None,
        }
    }

    /// Sets the mime type of the text resource content
    #[inline]
    pub fn with_mime(mut self, mime: impl Into<String>) -> Self {
        self.mime = Some(mime.into());
        self
    }
    
    /// Sets the title of the resource
    #[inline]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Sets annotations for the client
    pub fn with_annotations<F>(mut self, config: F) -> Self
    where
        F: FnOnce(Annotations) -> Annotations
    {
        self.annotations = Some(config(Default::default()));
        self
    }
}

impl JsonResourceContents {
    /// Creates a JSON resource content
    #[inline]
    pub fn new<T: Serialize>(uri: impl Into<Uri>, value: T) -> Self {
        Self {
            uri: uri.into(),
            value: serde_json::to_value(value).expect("Failed to serialize JSON"),
            mime: Some("text/plain".into()),
            title: None,
            annotations: None,
            meta: None,
        }
    }

    /// Sets the mime type of the JSON resource content
    #[inline]
    pub fn with_mime(mut self, mime: impl Into<String>) -> Self {
        self.mime = Some(mime.into());
        self
    }

    /// Sets the title of the resource
    #[inline]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Sets annotations for the client
    pub fn with_annotations<F>(mut self, config: F) -> Self
    where
        F: FnOnce(Annotations) -> Annotations
    {
        self.annotations = Some(config(Default::default()));
        self
    }
}

impl BlobResourceContents {
    /// Creates a blob resource content
    #[inline]
    pub fn new(uri: impl Into<Uri>, blob: impl Into<Bytes>) -> Self {
        Self {
            uri: uri.into(),
            blob: blob.into(),
            mime: None,
            title: None,
            annotations: None,
            meta: None,
        }
    }

    /// Sets the mime type of the blob resource content
    #[inline]
    pub fn with_mime(mut self, mime: impl Into<String>) -> Self {
        self.mime = Some(mime.into());
        self
    }

    /// Sets the title of the resource
    #[inline]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Sets annotations for the client
    pub fn with_annotations<F>(mut self, config: F) -> Self
    where
        F: FnOnce(Annotations) -> Annotations
    {
        self.annotations = Some(config(Default::default()));
        self
    }

    /// Returns audio data as a slice of bytes
    pub fn as_slice(&self) -> &[u8] {
        &self.blob
    }

    /// Turns this [`BlobResourceContents`] into a chunked stream of bytes
    pub fn into_stream(self) -> impl futures_util::Stream<Item = Bytes> {
        futures_util::stream::unfold(self.blob, |mut remaining_data| async move {
            if remaining_data.is_empty() {
                return None;
            }
            let chunk_size = remaining_data.len().min(CHUNK_SIZE);
            let chunk = remaining_data.split_to(chunk_size);
            Some((chunk, remaining_data))
        })
    }
}

impl EmptyResourceContents {
    /// Creates a empty resource content
    #[inline]
    pub fn new(uri: impl Into<Uri>) -> Self {
        Self {
            uri: uri.into(),
            title: None,
            mime: None,
            annotations: None,
            meta: None,
        }
    }
    /// Sets the mime type of the blob resource content
    #[inline]
    pub fn with_mime(mut self, mime: impl Into<String>) -> Self {
        self.mime = Some(mime.into());
        self
    }

    /// Sets the title of the resource
    #[inline]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
    
    /// Sets annotations for the client
    pub fn with_annotations<F>(mut self, config: F) -> Self
    where
        F: FnOnce(Annotations) -> Annotations
    {
        self.annotations = Some(config(Default::default()));
        self
    }
}

#[cfg(test)]
#[cfg(feature = "server")]
mod tests {
    use futures_util::StreamExt;
    use super::*;
    
    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct User {
        name: String,
        age: u8,
    }
    
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

        assert_eq!(json, r#"{"contents":[{"uri":"/res1","text":"test 1","mimeType":"plain/text"},{"uri":"/res1","text":"test 1","mimeType":"plain/text"}]}"#);
    }

    #[test]
    fn it_creates_result_from_array_of_str_tuples1() {
        let result = ReadResourceResult::from([
            TextResourceContents::new("/res1", "test 1"),
            TextResourceContents::new("/res1", "test 1")
        ]);

        let json = serde_json::to_string(&result).unwrap();

        assert_eq!(json, r#"{"contents":[{"uri":"/res1","text":"test 1","mimeType":"text/plain"},{"uri":"/res1","text":"test 1","mimeType":"text/plain"}]}"#);
    }

    #[test]
    fn it_creates_result_from_array_of_str_tuples2() {
        let result = ReadResourceResult::from([
            TextResourceContents::from(("/res1", "json", "test 1")),
            TextResourceContents::from(("/res1", "json", "test 1"))
        ]);

        let json = serde_json::to_string(&result).unwrap();

        assert_eq!(json, r#"{"contents":[{"uri":"/res1","text":"test 1","mimeType":"json"},{"uri":"/res1","text":"test 1","mimeType":"json"}]}"#);
    }

    #[test]
    fn it_creates_result_from_array_of_string_tuples1() {
        let result = ReadResourceResult::from([
            (String::from("/res1"), String::from("test 1")),
            (String::from("/res1"), String::from("test 1"))
        ]);

        let json = serde_json::to_string(&result).unwrap();

        assert_eq!(json, r#"{"contents":[{"uri":"/res1","text":"test 1","mimeType":"text/plain"},{"uri":"/res1","text":"test 1","mimeType":"text/plain"}]}"#);
    }

    #[test]
    fn it_creates_result_from_array_of_string_tuples2() {
        let result = ReadResourceResult::from([
            (String::from("/res1"), String::from("json"), String::from("test 1")),
            (String::from("/res1"), String::from("json"), String::from("test 1"))
        ]);

        let json = serde_json::to_string(&result).unwrap();

        assert_eq!(json, r#"{"contents":[{"uri":"/res1","text":"test 1","mimeType":"json"},{"uri":"/res1","text":"test 1","mimeType":"json"}]}"#);
    }

    #[test]
    fn it_creates_result_from_tuple_of_contents() {
        let content: ResourceContents = ("/res", "test").into();
        let result: ReadResourceResult = content.into();

        let json = serde_json::to_string(&result).unwrap();

        assert_eq!(json, r#"{"contents":[{"uri":"/res","text":"test","mimeType":"text/plain"}]}"#);
    }

    #[test]
    fn it_creates_result_from_objet_content() {
        let content = ResourceContents::new("/res")
            .with_json(User { name: "John".into(), age: 33 });
        
        let result: ReadResourceResult = content.into();

        let json = serde_json::to_string(&result).unwrap();

        assert_eq!(json, r#"{"contents":[{"uri":"/res","text":{"age":33,"name":"John"},"mimeType":"application/json"}]}"#);
    }

    #[test]
    fn it_tests_blob_content_serialization() {
        let blob = BlobResourceContents::new("file://hello", "hello world");

        let json = serde_json::to_string(&blob).expect("Should serialize");
        let deserialized: BlobResourceContents = serde_json::from_str(&json).expect("Should deserialize");

        assert_eq!(blob.blob, deserialized.blob);
        assert_eq!(blob.mime, deserialized.mime);
    }

    #[tokio::test]
    async fn it_tests_blob_content_into_stream_single_chunk() {
        // Test data that will be smaller than CHUNK_SIZE
        let test_data = "hello world";
        let blob = BlobResourceContents::new("file://hello", test_data.as_bytes());

        let stream = blob.into_stream();
        let mut stream = Box::pin(stream);
        let mut collected_data = Vec::new();

        while let Some(chunk) = stream.next().await {
            collected_data.extend_from_slice(&chunk);
        }

        let result_string = String::from_utf8(collected_data).expect("Should be valid UTF-8");
        assert_eq!(result_string, test_data);
    }

    #[tokio::test]
    async fn it_tests_blob_content_into_stream_multiple_chunks() {
        // Create data larger than CHUNK_SIZE to test chunking
        let test_data = "hello world".repeat(1000); // Much larger than 8192 bytes
        let blob = BlobResourceContents::new("file://hello", test_data.clone());

        let stream = blob.into_stream();
        let mut stream = Box::pin(stream);
        let mut collected_data = Vec::new();
        let mut chunk_count = 0;

        while let Some(chunk) = stream.next().await {
            collected_data.extend_from_slice(&chunk);
            chunk_count += 1;
        }

        let result_string = String::from_utf8(collected_data).expect("Should be valid UTF-8");
        assert_eq!(result_string, test_data);
        assert!(chunk_count > 1, "Should have multiple chunks for large data");
    }

    #[tokio::test]
    async fn it_tests_blob_content_into_stream_empty() {
        let blob = BlobResourceContents::new("file://hello", Bytes::new());

        let stream = blob.into_stream();
        let mut stream = Box::pin(stream);
        let result = stream.next().await;

        assert!(result.is_none(), "Empty data should produce no chunks");
    }

    #[tokio::test]
    async fn it_tests_blob_content_into_stream_exact_chunk_size() {
        // Create data exactly CHUNK_SIZE bytes
        let test_data = "a".repeat(CHUNK_SIZE);
        let blob = BlobResourceContents::new("file://hello", test_data.clone());

        let stream = blob.into_stream();
        let mut stream = Box::pin(stream);
        let mut collected_data = Vec::new();
        let mut chunk_count = 0;

        while let Some(chunk) = stream.next().await {
            collected_data.extend_from_slice(&chunk);
            chunk_count += 1;
        }

        let result_string = String::from_utf8(collected_data).expect("Should be valid UTF-8");
        assert_eq!(result_string, test_data);
        assert_eq!(chunk_count, 1, "Exactly CHUNK_SIZE data should produce one chunk");
    }
}
//! Types and utils for handling read resource results

use serde::{Deserialize, Serialize};
use base64::{engine::general_purpose, Engine};
#[cfg(feature = "server")]
use crate::{error::Error, types::{IntoResponse, RequestId, Response}};
use crate::types::{Annotations, Uri};

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
#[serde(untagged)]
pub enum ResourceContents {
    Text(TextResourceContents),
    Blob(BlobResourceContents),
    Empty(EmptyResourceContents),
}

/// Represents a blob resource content
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Serialize, Deserialize)]
pub struct BlobResourceContents {
    /// The URI of the resource.
    pub uri: Uri,

    /// The base64-encoded binary content of the resource.
    pub blob: String,

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
#[derive(Debug, Serialize, Deserialize)]
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

/// Represents an empty/unknown resource content
#[derive(Debug, Serialize, Deserialize)]
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
            Self::Text(ref text) => &text.uri,
            Self::Blob(ref blob) => &blob.uri,
            Self::Empty(ref empty) => &empty.uri
        }
    }

    /// Returns the URI of the resource content
    #[inline]
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(&text.text),
            Self::Blob(_) => None,
            Self::Empty(_) => None
        }
    }

    /// Returns the title of resource
    #[inline]
    pub fn title(&self) -> Option<&str> {
        match self {
            Self::Text(text) => text.title.as_deref(),
            Self::Blob(blob) => blob.title.as_deref(),
            Self::Empty(empty) => empty.title.as_deref()
        }
    }

    /// Returns the annotations of resource
    #[inline]
    pub fn annotations(&self) -> Option<&Annotations> {
        match self {
            Self::Text(text) => text.annotations.as_ref(),
            Self::Blob(blob) => blob.annotations.as_ref(),
            Self::Empty(empty) => empty.annotations.as_ref()
        }
    }

    /// Returns the URI of the resource content
    #[inline]
    pub fn blob(&self) -> Option<&str> {
        match self {
            Self::Blob(blob) => Some(&blob.blob),
            Self::Text(_) => None,
            Self::Empty(_) => None
        }
    }

    /// Returns the mime type of the resource content
    #[inline]
    pub fn mime(&self) -> Option<&str> {
        match self { 
            Self::Text(text) => text.mime.as_deref(),
            Self::Blob(blob) => blob.mime.as_deref(),
            Self::Empty(empty) => empty.mime.as_deref()
        }
    }
    
    /// Sets the mime type of the resource content
    #[inline]
    pub fn with_mime(mut self, mime: impl Into<String>) -> Self {
        match self {
            Self::Text(ref mut text) => text.mime = Some(mime.into()),
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
    pub fn with_blob(self, blob: impl AsRef<[u8]>) -> Self {
        let blob = general_purpose::STANDARD.encode(blob);
        match self {
            Self::Text(content) => Self::Blob(BlobResourceContents {
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

impl BlobResourceContents {
    /// Creates a blob resource content
    #[inline]
    pub fn new(uri: impl Into<Uri>, blob: impl AsRef<[u8]>) -> Self {
        Self {
            uri: uri.into(),
            blob: general_purpose::STANDARD.encode(blob),
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
}
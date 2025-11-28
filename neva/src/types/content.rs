//! Any Text, Image, Audio, Video content utilities

use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use bytes::Bytes;
use serde_json::Value;
use crate::shared;
use crate::error::{Error, ErrorCode};
use crate::types::helpers::{deserialize_base64_as_bytes, serialize_bytes_as_base64};
use crate::types::{
    CallToolResponse, 
    CallToolRequestParams,
    RequestParamsMeta,
    Annotations, 
    Resource, 
    ResourceContents, 
    Uri
};

const CHUNK_SIZE: usize = 8192;

/// Represents the content of the response.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Content {
    /// Audio content
    #[serde(rename = "audio")]
    Audio(AudioContent),
    
    /// Image content
    #[serde(rename = "image")]
    Image(ImageContent),
    
    /// Text content
    #[serde(rename = "text")]
    Text(TextContent),
    
    /// Resource link
    #[serde(rename = "resource_link")]
    ResourceLink(ResourceLink),
    
    /// Embedded resource
    #[serde(rename = "resource")]
    Resource(EmbeddedResource),

    /// Tool use content
    #[serde(rename = "tool_use")]
    ToolUse(ToolUse),

    /// Tool result content
    #[serde(rename = "tool_result")]
    ToolResult(ToolResult),
    
    /// Empty content
    #[serde(rename = "empty")]
    Empty(EmptyContent),
}

/// Represents an empty content object.
#[derive(Debug, Serialize, Deserialize)]
pub struct EmptyContent;

/// Text provided to or from an LLM.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema) for details
#[derive(Debug, Serialize, Deserialize)]
pub struct TextContent {
    /// The text content of the message.
    pub text: String,
    
    /// Optional annotations for the client.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,

    /// Metadata reserved by MCP for protocol-level metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

/// Audio provided to or from an LLM.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema) for details
#[derive(Debug, Serialize, Deserialize)]
pub struct AudioContent {
    /// Raw audio data.
    ///
    /// **Note:** will be serialized as a base64-encoded string
    #[serde(
        serialize_with = "serialize_bytes_as_base64",
        deserialize_with = "deserialize_base64_as_bytes")]
    pub data: Bytes,

    /// The MIME type of the audio content, e.g. "audio/mpeg" or "audio/wav".
    #[serde(rename = "mimeType")]
    pub mime: String,
    
    /// Optional annotations for the client.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,

    /// Metadata reserved by MCP for protocol-level metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

/// An image provided to or from an LLM.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema) for details
#[derive(Debug, Serialize, Deserialize)]
pub struct ImageContent {
    /// Raw image data.
    /// 
    /// **Note:** will be serialized as a base64-encoded string
    #[serde(
        serialize_with = "serialize_bytes_as_base64", 
        deserialize_with = "deserialize_base64_as_bytes")]
    pub data: Bytes,

    /// The MIME type of the audio content, e.g. "image/jpg" or "image/png".
    #[serde(rename = "mimeType")]
    pub mime: String,

    /// Optional annotations for the client.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,

    /// Metadata reserved by MCP for protocol-level metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

/// A resource that the server is capable of reading, included in a prompt or tool call result.
/// 
/// **Note:** resource links returned by tools are not guaranteed to appear in the results of `resources/list` requests.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema) for details
#[derive(Debug, Serialize, Deserialize)]
pub struct ResourceLink {
    /// The URI of this resource.
    pub uri: Uri,
    
    /// Intended for programmatic or logical use  
    /// but used as a display name in past specs or fallback (if a title isn't present).
    pub name: String,

    /// The resource size in bytes, if known
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<usize>,

    /// The MIME type of the resource. If known.
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    
    /// Intended for UI and end-user contexts - optimized to be human-readable and easily understood,
    /// even by those unfamiliar with domain-specific terminology.
    /// 
    /// If not provided, the name should be used for display (except for Tool,
    /// where `annotations.title` should be given precedence over using `name`, if present).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A description of what this resource represents.
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,
    
    /// Optional annotations for the client.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,

    /// Metadata reserved by MCP for protocol-level metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

/// The contents of a resource, embedded into a prompt or tool call result.
/// 
/// It is up to the client how best to render embedded resources for the benefit
/// of the LLM and/or the user.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema) for details
#[derive(Debug, Serialize, Deserialize)]
pub struct EmbeddedResource {
    /// The resource content of the message.
    pub resource: ResourceContents,
    
    /// Optional annotations for the client.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,

    /// Metadata reserved by MCP for protocol-level metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

/// Represents a request from the assistant to call a tool.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema) for details
#[derive(Debug, Serialize, Deserialize)]
pub struct ToolUse {
    /// A unique identifier for this tool use.
    /// 
    /// This ID is used to match tool results to their corresponding tool uses.
    pub id: String,

    /// The name of the tool to call.
    pub name: String,

    /// The arguments to pass to the tool, conforming to the tool's input schema.
    pub input: Option<HashMap<String, Value>>,

    /// Metadata reserved by MCP for protocol-level metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<RequestParamsMeta>,
}

/// Represents the result of a tool use, provided by the user back to the assistant.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema) for details
#[derive(Debug, Serialize, Deserialize)]
pub struct ToolResult {
    /// The ID of the tool use this result corresponds to.
    /// 
    /// This **MUST** match the ID from a previous [`ToolUse`].
    #[serde(rename = "toolUseId")]
    pub tool_use_id: String,

    /// The unstructured result content of the tool use.
    /// 
    /// This has the same format as [`CallToolResponse::content`] and can include text, images, audio, resource links, and embedded resources.
    pub content: Vec<Content>,
    
    /// An optional JSON object that represents the structured result of the tool call.
    /// 
    /// If the tool defined an `outputSchema`, this **SHOULD** conform to that schema.
    #[serde(rename = "structuredContent", skip_serializing_if = "Option::is_none")]
    pub struct_content: Option<serde_json::Value>,

    /// Whether the tool call was unsuccessful.
    /// 
    /// If true, the content typically describes the error that occurred.
    /// 
    /// Default: `false`
    #[serde(default, rename = "isError")]
    pub is_error: bool,

    /// Metadata reserved by MCP for protocol-level metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

impl From<&str> for Content {
    #[inline]
    fn from(value: &str) -> Self {
        Self::text(value)
    }
}

impl From<String> for Content {
    #[inline]
    fn from(value: String) -> Self {
        Self::text(value)
    }
}

impl From<Resource> for ResourceLink {
    #[inline]
    fn from(res: Resource) -> Self {
        Self {
            name: res.name,
            uri: res.uri,
            size: res.size,
            mime: res.mime,
            title: res.title,
            descr: res.descr,
            annotations: res.annotations,
            meta: res.meta,
        }
    }
}

impl From<Resource> for Content {
    #[inline]
    fn from(res: Resource) -> Self {
        Self::ResourceLink(res.into())
    }
}

impl From<serde_json::Value> for Content {
    #[inline]
    fn from(value: serde_json::Value) -> Self {
        Self::Text(TextContent::new(value.to_string()))
    }
}

impl From<TextContent> for Content {
    #[inline]
    fn from(value: TextContent) -> Self {
        Self::Text(value)
    }
}

impl TryFrom<Content> for TextContent {
    type Error = Error;

    #[inline]
    fn try_from(value: Content) -> Result<Self, Self::Error> {
        match value {
            Content::Text(text) => Ok(text),
            _ => Err(Error::new(ErrorCode::InternalError, "Invalid content type")),
        }
    }
}

impl<'a> TryFrom<&'a Content> for &'a TextContent {
    type Error = Error;

    #[inline]
    fn try_from(value: &'a Content) -> Result<Self, Self::Error> {
        match value {
            Content::Text(text) => Ok(text),
            _ => Err(Error::new(ErrorCode::InternalError, "Invalid content type")),
        }
    }
}

impl From<AudioContent> for Content {
    #[inline]
    fn from(value: AudioContent) -> Self {
        Self::Audio(value)
    }
}

impl TryFrom<Content> for AudioContent {
    type Error = Error;

    #[inline]
    fn try_from(value: Content) -> Result<Self, Self::Error> {
        match value {
            Content::Audio(audio) => Ok(audio),
            _ => Err(Error::new(ErrorCode::InternalError, "Invalid content type")),
        }
    }
}

impl From<ImageContent> for Content {
    #[inline]
    fn from(value: ImageContent) -> Self {
        Self::Image(value)
    }
}

impl TryFrom<Content> for ImageContent {
    type Error = Error;

    #[inline]
    fn try_from(value: Content) -> Result<Self, Self::Error> {
        match value {
            Content::Image(img) => Ok(img),
            _ => Err(Error::new(ErrorCode::InternalError, "Invalid content type")),
        }
    }
}

impl From<ResourceLink> for Content {
    #[inline]
    fn from(value: ResourceLink) -> Self {
        Self::ResourceLink(value)
    }
}

impl TryFrom<Content> for ResourceLink {
    type Error = Error;

    #[inline]
    fn try_from(value: Content) -> Result<Self, Self::Error> {
        match value {
            Content::ResourceLink(res) => Ok(res),
            _ => Err(Error::new(ErrorCode::InternalError, "Invalid content type")),
        }
    }
}

impl From<EmbeddedResource> for Content {
    #[inline]
    fn from(value: EmbeddedResource) -> Self {
        Self::Resource(value)
    }
}

impl TryFrom<Content> for EmbeddedResource {
    type Error = Error;

    #[inline]
    fn try_from(value: Content) -> Result<Self, Self::Error> {
        match value { 
            Content::Resource(res) => Ok(res),
            _ => Err(Error::new(ErrorCode::InternalError, "Invalid content type")),
        }
    }
}

impl From<ToolUse> for Content {
    #[inline]
    fn from(value: ToolUse) -> Self {
        Self::ToolUse(value)
    }
}

impl From<ToolResult> for Content {
    #[inline]
    fn from(value: ToolResult) -> Self {
        Self::ToolResult(value)
    }
}

impl From<ToolUse> for CallToolRequestParams {
    #[inline]
    fn from(value: ToolUse) -> Self {
        Self { 
            name: value.name, 
            args: value.input, 
            meta: value.meta
        }
    }
}

impl TryFrom<Content> for ToolUse {
    type Error = Error;

    #[inline]
    fn try_from(value: Content) -> Result<Self, Self::Error> {
        match value { 
            Content::ToolUse(tool_use) => Ok(tool_use),
            _ => Err(Error::new(ErrorCode::InternalError, "Invalid content type")),
        }
    }
}

impl TryFrom<Content> for ToolResult {
    type Error = Error;

    #[inline]
    fn try_from(value: Content) -> Result<Self, Self::Error> {
        match value { 
            Content::ToolResult(tool_result) => Ok(tool_result),
            _ => Err(Error::new(ErrorCode::InternalError, "Invalid content type")),
        }
    }
}

impl Content {
    /// Creates a text [`Content`]
    #[inline]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(TextContent::new(text))
    }

    /// Creates a JSON [`Content`]
    #[inline]
    pub fn json<T: Serialize>(json: T) -> Self {
        let json = serde_json::to_value(json).unwrap();
        Self::from(json)
    }
    
    /// Creates an image [`Content`]
    #[inline]
    pub fn image(data: impl Into<Bytes>) -> Self {
        Self::Image(ImageContent::new(data))
    }

    /// Creates an audio [`Content`]
    #[inline]
    pub fn audio(data: impl Into<Bytes>) -> Self {
        Self::Audio(AudioContent::new(data))
    }
    
    /// Creates an embedded resource [`Content`]
    #[inline]
    pub fn resource(resource: impl Into<ResourceContents>) -> Self {
        Self::Resource(EmbeddedResource::new(resource))
    }
    
    /// Creates a resource link [`Content`]
    #[inline]
    pub fn link(resource: impl Into<Resource>) -> Self {
        resource.into().into()
    }

    /// Creates a tool result [`Content`]
    #[inline]
    pub fn tool_result(id: String, resp: CallToolResponse) -> Self {
        Self::ToolResult(ToolResult::new(id, resp))
    }

    /// Creates a tool use [`Content`]
    #[inline]
    pub fn tool_use<N, Args>(name: N, args: Args) -> Self
    where
        N: Into<String>,
        Args: shared::IntoArgs
    {
        Self::ToolUse(ToolUse::new(name, args))
    }

    /// Creates an empty [`Content`]
    #[inline]
    pub fn empty() -> Self {
        Self::Empty(EmptyContent)
    }
    
    /// Returns the type of the content.
    #[inline]
    pub fn get_type(&self) -> &str {
        match self { 
            Self::Empty(_) => "empty",
            Self::Audio(_) => "audio",
            Self::Image(_) => "image",
            Self::Text(_) => "text",
            Self::ResourceLink(_) => "resource_link",
            Self::Resource(_) => "resource",
            Self::ToolUse(_) => "tool_use",
            Self::ToolResult(_) => "tool_result"
        }
    }
    
    /// Returns the content as a text content.
    #[inline]
    pub fn as_text(&self) -> Option<&TextContent> {
        match self {
            Self::Text(c) => Some(c),
            _ => None
        }
    }
    
    /// Returns the content as a deserialized struct
    #[inline]
    pub fn as_json<T: DeserializeOwned>(&self) -> Option<T> {
        match self { 
            Self::Text(c) => serde_json::from_str(&c.text).ok(),
            _ => None
        }
    }

    /// Returns the content as an audio content.
    #[inline]
    pub fn as_audio(&self) -> Option<&AudioContent> {
        match self {
            Self::Audio(c) => Some(c),
            _ => None
        }
    }

    /// Returns the content as an image.
    #[inline]
    pub fn as_image(&self) -> Option<&ImageContent> {
        match self {
            Self::Image(c) => Some(c),
            _ => None
        }
    }

    /// Returns the content as a resource link.
    #[inline]
    pub fn as_link(&self) -> Option<&ResourceLink> {
        match self {
            Self::ResourceLink(c) => Some(c),
            _ => None
        }
    }

    /// Returns the content as an embedded resource.
    #[inline]
    pub fn as_resource(&self) -> Option<&EmbeddedResource> {
        match self {
            Self::Resource(c) => Some(c),
            _ => None
        }
    }
}

impl TextContent {
    /// Creates a new [`TextContent`]
    #[inline]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            annotations: None,
            meta: None
        }
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

impl AudioContent {
    /// Creates a new [`AudioContent`]
    #[inline]
    pub fn new(data: impl Into<Bytes>) -> Self {
        Self {
            data: data.into(),
            mime: "audio/wav".into(),
            annotations: None,
            meta: None
        }
    }

    /// Sets a mime type for the audio content
    pub fn with_mime(mut self, mime: impl Into<String>) -> Self {
        self.mime = mime.into();
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
        &self.data
    }

    /// Turns this [`AudioContent`] into a chunked stream of bytes
    pub fn into_stream(self) -> impl futures_util::Stream<Item = Bytes> {
        futures_util::stream::unfold(self.data, |mut remaining_data| async move {
            if remaining_data.is_empty() {
                return None;
            }
            let chunk_size = remaining_data.len().min(CHUNK_SIZE);
            let chunk = remaining_data.split_to(chunk_size);
            Some((chunk, remaining_data))
        })
    }
}

impl ImageContent {
    /// Creates a new [`ImageContent`]
    #[inline]
    pub fn new(data: impl Into<Bytes>) -> Self {
        Self {
            data: data.into(),
            mime: "image/jpg".into(),
            annotations: None,
            meta: None
        }
    }

    /// Sets a mime type for the image content
    pub fn with_mime(mut self, mime: impl Into<String>) -> Self {
        self.mime = mime.into();
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
        &self.data
    }

    /// Turns this [`ImageContent`] into a chunked stream of bytes
    pub fn into_stream(self) -> impl futures_util::Stream<Item = Bytes> {
        futures_util::stream::unfold(self.data, |mut remaining_data| async move {
            if remaining_data.is_empty() {
                return None;
            }
            let chunk_size = remaining_data.len().min(CHUNK_SIZE);
            let chunk = remaining_data.split_to(chunk_size);
            Some((chunk, remaining_data))
        })
    }
}

impl ResourceLink {
    /// Creates a new [`ResourceLink`] content
    pub fn new(resource: impl Into<Resource>) -> Self {
        Self::from(resource.into())
    }
}

impl EmbeddedResource {
    /// Creates a new [`EmbeddedResource`] content
    #[inline]
    pub fn new(resource: impl Into<ResourceContents>) -> Self {
        Self {
            resource: resource.into(),
            annotations: None,
            meta: None
        }
    }
}

impl ToolUse {
    /// Creates a new [`ToolUse`] content
    #[inline]
    pub fn new<N, Args>(name: N, args: Args) -> Self
    where
        N: Into<String>,
        Args: shared::IntoArgs
    {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            input: args.into_args(),
            meta: None
        }
    }
}

impl ToolResult {
    /// Creates a new [`ToolResult`] content
    #[inline]
    pub fn new(id: String, resp: CallToolResponse) -> Self {
        Self {
            tool_use_id: id,
            content: resp.content,
            struct_content: resp.struct_content,
            is_error: resp.is_error,
            meta: None
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use futures_util::StreamExt;
    
    #[derive(Deserialize)]
    struct Test {
        name: String,
        age: u32
    }
    
    #[test]
    fn it_serializes_text_content_to_json() {
        let content = Content::text("hello world");
        let json = serde_json::to_string(&content).unwrap();
        
        assert_eq!(json, r#"{"type":"text","text":"hello world"}"#);
    }

    #[test]
    fn it_deserializes_text_content_to_json() {
        let json = r#"{"type":"text","text":"hello world"}"#;
        let content = serde_json::from_str::<Content>(json)
            .unwrap();

        assert_eq!(content.as_text().unwrap().text, "hello world");
    }

    #[test]
    fn it_deserializes_structures_text_content_to_json() {
        let json = r#"{"type":"text","text":"{\"name\":\"John\",\"age\":30}"}"#;
        let content = serde_json::from_str::<Content>(json)
            .unwrap();

        let user: Test = content.as_json().unwrap();
        
        assert_eq!(user.name, "John");
        assert_eq!(user.age, 30);
    }

    #[test]
    fn it_serializes_audio_content_to_json() {
        let content = Content::audio("hello world");
        let json = serde_json::to_string(&content).unwrap();

        assert_eq!(json, r#"{"type":"audio","data":"aGVsbG8gd29ybGQ=","mimeType":"audio/wav"}"#);
    }

    #[test]
    fn it_deserializes_audio_content_to_json() {
        let json = r#"{"type":"audio","data":"aGVsbG8gd29ybGQ=","mimeType":"audio/wav"}"#;
        let content = serde_json::from_str::<Content>(json)
            .unwrap();

        assert_eq!(String::from_utf8_lossy(content.as_audio().unwrap().as_slice()), "hello world");
        assert_eq!(content.as_audio().unwrap().mime, "audio/wav");
    }

    #[test]
    fn it_serializes_image_content_to_json() {
        let content = Content::image("hello world");
        let json = serde_json::to_string(&content).unwrap();

        assert_eq!(json, r#"{"type":"image","data":"aGVsbG8gd29ybGQ=","mimeType":"image/jpg"}"#);
    }

    #[test]
    fn it_deserializes_image_content_to_json() {
        let json = r#"{"type":"image","data":"aGVsbG8gd29ybGQ=","mimeType":"image/jpg"}"#;
        let content = serde_json::from_str::<Content>(json)
            .unwrap();

        assert_eq!(String::from_utf8_lossy(content.as_image().unwrap().as_slice()), "hello world");
        assert_eq!(content.as_image().unwrap().mime, "image/jpg");
    }

    #[test]
    #[cfg(feature = "server")]
    fn it_serializes_resource_content_to_json() {
        let content = Content::resource(ResourceContents::new("res://resource")
            .with_text("hello world")
            .with_title("some resource")
            .with_annotations(|a| a
                .with_audience("user")
                .with_priority(1.0)));
        
        let json = serde_json::to_string(&content).unwrap();

        assert_eq!(json, r#"{"type":"resource","resource":{"uri":"res://resource","text":"hello world","title":"some resource","mimeType":"text/plain","annotations":{"audience":["user"],"priority":1.0}}}"#);
    }

    #[test]
    #[cfg(feature = "server")]
    fn it_deserializes_resource_content_to_json() {
        use crate::types::Role;
        
        let json = r#"{"type":"resource","resource":{"uri":"res://resource","text":"hello world","title":"some resource","mimeType":"text/plain","annotations":{"audience":["user"],"priority":1.0}}}"#;
        let content = serde_json::from_str::<Content>(json)
            .unwrap();

        let res = &content.as_resource().unwrap().resource;
        
        assert_eq!(res.uri().to_string(), "res://resource");
        assert_eq!(res.mime().unwrap(), "text/plain");
        assert_eq!(res.text().unwrap(), "hello world");
        assert_eq!(res.title().unwrap(), "some resource");
        assert_eq!(res.annotations().unwrap().audience, [Role::User]);
        assert_eq!(res.annotations().unwrap().priority, 1.0);
    }

    #[test]
    #[cfg(feature = "server")]
    fn it_serializes_resource_link_content_to_json() {
        let content = Content::link(Resource::new("res://resource", "some resource")
            .with_title("some resource")
            .with_descr("some resource")
            .with_size(2)
            .with_annotations(|a| a
                .with_audience("user")
                .with_priority(1.0)));

        let json = serde_json::to_string(&content).unwrap();

        assert_eq!(json, r#"{"type":"resource_link","uri":"res://resource","name":"some resource","size":2,"title":"some resource","description":"some resource","annotations":{"audience":["user"],"priority":1.0}}"#);
    }

    #[test]
    #[cfg(feature = "server")]
    fn it_deserializes_resource_link_content_to_json() {
        use crate::types::Role;
        
        let json = r#"{"type":"resource_link","uri":"res://resource","name":"some resource","size":2,"title":"some resource","description":"some resource","annotations":{"audience":["user"],"priority":1.0}}"#;
        let content = serde_json::from_str::<Content>(json)
            .unwrap();

        let res = content.as_link().unwrap();

        assert_eq!(res.uri.to_string(), "res://resource");
        assert_eq!(res.name, "some resource");
        assert_eq!(res.mime.as_deref(), None);
        assert_eq!(res.title.as_deref(), Some("some resource"));
        assert_eq!(res.annotations.as_ref().unwrap().audience, [Role::User]);
        assert_eq!(res.annotations.as_ref().unwrap().priority, 1.0);
    }

    #[tokio::test]
    async fn it_tests_audio_content_into_stream_single_chunk() {
        // Test data that will be smaller than CHUNK_SIZE
        let test_data = "hello world";
        let audio = AudioContent::new(test_data.as_bytes());

        let stream = audio.into_stream();
        let mut stream = Box::pin(stream);
        let mut collected_data = Vec::new();

        while let Some(chunk) = stream.next().await {
            collected_data.extend_from_slice(&chunk);
        }

        let result_string = String::from_utf8(collected_data).expect("Should be valid UTF-8");
        assert_eq!(result_string, test_data);
    }

    #[tokio::test]
    async fn it_tests_audio_content_into_stream_multiple_chunks() {
        // Create data larger than CHUNK_SIZE to test chunking
        let test_data = "hello world".repeat(1000); // Much larger than 8192 bytes
        let audio = AudioContent::new(test_data.clone());

        let stream = audio.into_stream();
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
    async fn it_tests_audio_content_into_stream_empty() {
        let audio = AudioContent::new(Bytes::new());

        let stream = audio.into_stream();
        let mut stream = Box::pin(stream);
        let result = stream.next().await;

        assert!(result.is_none(), "Empty data should produce no chunks");
    }

    #[tokio::test]
    async fn it_tests_audio_content_into_stream_exact_chunk_size() {
        // Create data exactly CHUNK_SIZE bytes
        let test_data = "a".repeat(CHUNK_SIZE);
        let audio = AudioContent::new(test_data.clone());

        let stream = audio.into_stream();
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

    #[tokio::test]
    async fn it_tests_image_content_into_stream_single_chunk() {
        let test_data = "hello world";
        let image = ImageContent::new(test_data.as_bytes());

        let stream = image.into_stream();
        let mut stream = Box::pin(stream);
        let mut collected_data = Vec::new();

        while let Some(chunk) = stream.next().await {
            collected_data.extend_from_slice(&chunk);
        }

        let result_string = String::from_utf8(collected_data).expect("Should be valid UTF-8");
        assert_eq!(result_string, test_data);
    }

    #[tokio::test]
    async fn it_tests_image_content_into_stream_multiple_chunks() {
        let test_data = "hello world".repeat(1000);
        let image = ImageContent::new(test_data.clone());

        let stream = image.into_stream();
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
    async fn it_tests_stream_chunk_sizes() {
        let test_data = "hello world".repeat(500); // Create data that will span multiple chunks
        let audio = AudioContent::new(test_data.clone());

        let stream = audio.into_stream();
        let mut stream = Box::pin(stream);
        let mut total_size = 0;
        let mut max_chunk_size = 0;

        while let Some(chunk) = stream.next().await {
            let chunk_size = chunk.len();
            total_size += chunk_size;
            max_chunk_size = max_chunk_size.max(chunk_size);

            // Each chunk should not exceed CHUNK_SIZE
            assert!(chunk_size <= CHUNK_SIZE, "Chunk size should not exceed CHUNK_SIZE");
        }

        assert_eq!(total_size, test_data.len());
        assert!(max_chunk_size <= CHUNK_SIZE);
    }

    #[tokio::test]
    async fn it_tests_stream_preserves_binary_data() {
        let test_data: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        let audio = AudioContent::new(test_data.clone());

        let stream = audio.into_stream();
        let mut stream = Box::pin(stream);
        let mut collected_data = Vec::new();

        while let Some(chunk) = stream.next().await {
            collected_data.extend_from_slice(&chunk);
        }

        assert_eq!(collected_data, test_data);
    }

    #[tokio::test]
    async fn it_tests_audio_content_collect_alternative() {
        let test_data = "hello world";
        let audio = AudioContent::new(test_data.as_bytes());

        let stream = audio.into_stream();
        let chunks: Vec<_> = tokio_stream::StreamExt::collect::<Vec<_>>(stream).await;

        let collected_data: Vec<u8> = chunks.into_iter().flatten().collect();

        let result_string = String::from_utf8(collected_data).expect("Should be valid UTF-8");
        assert_eq!(result_string, test_data);
    }

    #[test]
    fn it_tests_audio_content_serialization() {
        let audio = AudioContent::new("hello world");

        let json = serde_json::to_string(&audio).expect("Should serialize");
        let deserialized: AudioContent = serde_json::from_str(&json).expect("Should deserialize");

        assert_eq!(audio.data, deserialized.data);
        assert_eq!(audio.mime, deserialized.mime);
    }
}
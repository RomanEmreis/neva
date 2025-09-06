//! Any Text, Image, Audio, Video content utilities

use serde::{Deserialize, Serialize};
use crate::error::{Error, ErrorCode};
use crate::types::{Annotations, Resource, ResourceContents, Uri};

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
    /// The base64-encoded audio data.
    pub data: String,

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
    /// The base64-encoded image data.
    pub data: String,

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
        Self::json(value)
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

impl Content {
    /// Creates a text [`Content`]
    #[inline]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(TextContent::new(text))
    }

    /// Creates a JSON [`Content`]
    #[inline]
    pub fn json(json: serde_json::Value) -> Self {
        Self::Text(TextContent::new(json.to_string()))
    }
    
    /// Creates an image [`Content`]
    #[inline]
    pub fn image(data: impl Into<String>) -> Self {
        Self::Image(ImageContent::new(data))
    }

    /// Creates an audio [`Content`]
    #[inline]
    pub fn audio(data: impl Into<String>) -> Self {
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
    pub fn new(data: impl Into<String>) -> Self {
        Self {
            data: data.into(),
            mime: "audio/wav".into(),
            annotations: None,
            meta: None
        }
    }

    /// Sets mime type for the audio content
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
}

impl ImageContent {
    /// Creates a new [`ImageContent`]
    #[inline]
    pub fn new(data: impl Into<String>) -> Self {
        Self {
            data: data.into(),
            mime: "image/jpg".into(),
            annotations: None,
            meta: None
        }
    }

    /// Sets mime type for the image content
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

#[cfg(test)]
mod test {
    use super::*;
    
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
    fn it_serializes_audio_content_to_json() {
        let content = Content::audio("hello world");
        let json = serde_json::to_string(&content).unwrap();

        assert_eq!(json, r#"{"type":"audio","data":"hello world","mimeType":"audio/wav"}"#);
    }

    #[test]
    fn it_deserializes_audio_content_to_json() {
        let json = r#"{"type":"audio","data":"hello world","mimeType":"audio/wav"}"#;
        let content = serde_json::from_str::<Content>(json)
            .unwrap();

        assert_eq!(content.as_audio().unwrap().data, "hello world");
        assert_eq!(content.as_audio().unwrap().mime, "audio/wav");
    }

    #[test]
    fn it_serializes_image_content_to_json() {
        let content = Content::image("hello world");
        let json = serde_json::to_string(&content).unwrap();

        assert_eq!(json, r#"{"type":"image","data":"hello world","mimeType":"image/jpg"}"#);
    }

    #[test]
    fn it_deserializes_image_content_to_json() {
        let json = r#"{"type":"image","data":"hello world","mimeType":"image/jpg"}"#;
        let content = serde_json::from_str::<Content>(json)
            .unwrap();

        assert_eq!(content.as_image().unwrap().data, "hello world");
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
}
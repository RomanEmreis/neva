//! Any Text, Image, Audio, Video content utilities

use serde::{Deserialize, Serialize};
use crate::types::ResourceContents;

/// Represents the content of response.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Serialize, Deserialize)]
pub struct Content {
    /// The type of content. This determines the structure of the content object. 
    /// 
    /// Can be "image", "audio", "text", "resource".
    #[serde(rename = "type")]
    pub r#type: String,
    
    /// The text content of the message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    
    /// The base64-encoded image data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    
    /// The MIME type of the image.
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    
    /// The resource content of the message (if embedded).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<ResourceContents>
}

impl Content {
    /// Creates a text [`Content`]
    #[inline]
    pub fn text(text: &str) -> Self {
        Self {
            text: Some(text.into()),
            r#type: "text".into(),
            mime: None,
            data: None,
            resource: None
        }
    }
}


//! Any Text, Image, Audio, Video content utilities

use serde::{Deserialize, Serialize};
use crate::types::ResourceContents;

/// Represents the content of response.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Debug, Serialize, Deserialize)]
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

impl From<&str> for Content {
    #[inline]
    fn from(value: &str) -> Self {
        Self::text(value)
    }
}

impl From<String> for Content {
    #[inline]
    fn from(value: String) -> Self {
        Self {
            text: Some(value),
            r#type: "text".into(),
            mime: Some("text/plain".into()),
            data: None,
            resource: None
        }
    }
}

impl From<serde_json::Value> for Content {
    #[inline]
    fn from(value: serde_json::Value) -> Self {
        Self::json(value)
    }
}

impl Content {
    /// Creates a text [`Content`]
    #[inline]
    pub fn text(text: &str) -> Self {
        Self {
            text: Some(text.into()),
            r#type: "text".into(),
            mime: Some("text/plain".into()),
            data: None,
            resource: None
        }
    }

    /// Creates a JSON [`Content`]
    #[inline]
    pub fn json(json: serde_json::Value) -> Self {
        Self {
            text: Some(json.to_string()),
            r#type: "text".into(),
            mime: Some("application/json".into()),
            data: None,
            resource: None
        }
    }

    /// Creates an empty [`Content`]
    #[inline]
    pub fn empty() -> Self {
        Self {
            text: None,
            r#type: "text".into(),
            mime: None,
            data: None,
            resource: None
        }
    }
}


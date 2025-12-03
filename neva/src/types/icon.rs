//! Types and utilities for icons

use serde::{Serialize, Deserialize, Serializer, Deserializer};
use crate::types::Uri;

/// Represents an optionally sized icon that can be displayed in a user interface.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct Icon {
    /// Optional MIME type override if the source MIME type is missing or generic.
    /// 
    /// For example, `"image/png"`, `"image/jpeg"`, or `"image/svg+xml"`.
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    
    /// Optional array of strings that specify sizes at which the icon can be used.
    /// Each string should be in WxH format (e.g., `"48x48"`, `"96x96"`) or `"any"` 
    /// for scalable formats like SVG.
    /// 
    /// If not provided, the client should assume that the icon can be used at any size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sizes: Option<Vec<IconSize>>,
    
    /// A standard URI pointing to an icon resource. Maybe an HTTP/HTTPS URL or a 
    /// `data:` URI with Base64-encoded image data.
    /// 
    /// Consumers **SHOULD** take steps to ensure URLs serving icons are from the 
    /// same domain as the client/server or a trusted domain.
    /// 
    /// Consumers **SHOULD** take appropriate precautions when consuming SVGs as they can contain
    /// executable JavaScript.
    pub src: Uri,
    
    /// Optional specifier for the theme this icon is designed for. `light` indicates
    /// the icon is designed to be used with a light background, and `dark` indicates
    /// the icon is designed to be used with a dark background.
    /// 
    /// If not provided, the client should assume the icon can be used with any theme.
    pub theme: Option<IconTheme>
}

/// Represents the theme the icon is designed for.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IconTheme {
    /// The icon is designed for use with a dark background.
    Dark,
    
    /// The icon is designed for use with a light background.
    Light
}

/// Represents the size of an icon.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct IconSize {
    /// The width of the icon in pixels.
    width: usize,
    
    /// The height of the icon in pixels.
    height: usize,
    
    /// Indicates whether the icon is scalable (e.g., SVG).
    /// If `true` the `width` and `height` fields should be ignored.
    is_any: bool
}

impl Serialize for IconSize {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where 
        S: Serializer
    {
        if self.is_any {
            serializer.serialize_str("any")
        } else {
            serializer.collect_str(&format!("{}x{}", self.width, self.height))
        }
    }
}

impl<'de> Deserialize<'de> for IconSize {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(IconSize::from(s))
    }
}

impl From<String> for IconSize {
    #[inline]
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

impl From<&str> for IconSize {
    #[inline]
    fn from(value: &str) -> Self {
        match value {
            "any" => Self { width: 0, height: 0, is_any: true },
            s => {
                let mut parts = s.split('x');
                Self {
                    width: parts.next().map(|p| p
                        .parse()
                        .unwrap_or(0))
                        .unwrap_or(0),
                    height: parts.next().map(|p| p.parse()
                        .unwrap_or(0))
                        .unwrap_or(0),
                    is_any: false
                }
            }
        }
    }
}

impl IconSize {
    /// Creates a new [`IconSize`] with exact width and height
    #[inline]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            is_any: false
        }
    }
    
    /// Creates a new [`IconSize`] that can be used with any size
    #[inline]
    pub fn any() -> Self {
        Self {
            width: 0,
            height: 0,
            is_any: true
        }
    }
}

impl Icon {
    /// Creates a new [`Icon`] with the specified URL
    #[inline]
    pub fn new(url: impl Into<Uri>) -> Self {
        Self {
            src: url.into(),
            mime: None,
            sizes: None,
            theme: None
        }
    }
    
    /// Sets the MIME type
    #[inline]
    pub fn with_mime(mut self, mime: impl Into<String>) -> Self {
        self.mime = Some(mime.into());
        self
    }

    /// Sets the icon sizes
    #[inline]
    pub fn with_sizes(mut self, sizes: impl IntoIterator<Item = IconSize>) -> Self {
        self.sizes = Some(sizes.into_iter().collect());
        self
    }

    /// Sets the icon theme
    #[inline]
    pub fn with_theme(mut self, theme: IconTheme) -> Self {
        self.theme = Some(theme);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    fn create_test_icon() -> Icon {
        Icon {
            mime: Some("image/png".into()),
            sizes: Some(vec![IconSize::new(48, 48)]),
            src: Uri::from("https://example.com/icon.png"),
            theme: Some(IconTheme::Dark)
        }
    }
    
    #[test]
    fn it_converts_icon_size_from_str() {
        let size = IconSize::from("48x48");
        
        assert_eq!(size.width, 48);
        assert_eq!(size.height, 48);
        assert!(!size.is_any);
    }

    #[test]
    fn it_converts_icon_size_from_string() {
        let size = IconSize::from(String::from("48x48"));

        assert_eq!(size.width, 48);
        assert_eq!(size.height, 48);
        assert!(!size.is_any);
    }

    #[test]
    fn it_converts_icon_size_any() {
        let size = IconSize::from("any");

        assert_eq!(size.width, 0);
        assert_eq!(size.height, 0);
        assert!(size.is_any);
    }

    #[test]
    fn it_converts_icon_from_invalid_string() {
        let size = IconSize::from("sdsd");

        assert_eq!(size.width, 0);
        assert_eq!(size.height, 0);
        assert!(!size.is_any);
    }
    
    #[test]
    fn it_serializes_icon_sizes() {
        let size = [
            IconSize::any(),
            IconSize::new(48, 48),
            IconSize::new(128, 128)
        ];
        
        let serialized = serde_json::to_string(&size).unwrap();
        
        assert_eq!(serialized, r#"["any","48x48","128x128"]"#);
    }
    
    #[test]
    fn it_deserializes_icon_sizes() {
        let deserialized: Vec<IconSize> = serde_json::from_str(r#"["any","48x48","128x128"]"#)
            .unwrap();
        
        assert_eq!(deserialized, [
            IconSize::any(),
            IconSize::new(48, 48),
            IconSize::new(128, 128)
        ]);
    }
    
    #[test]
    fn it_serializes_icon() {
        let icon = create_test_icon();
        let serialized = serde_json::to_string(&icon).unwrap();
        
        assert_eq!(
            serialized, 
            r#"{"mimeType":"image/png","sizes":["48x48"],"src":"https://example.com/icon.png","theme":"dark"}"#);
    }
    
    #[test]
    fn it_deserializes_icon() {
        let json = r#"{"mimeType":"image/png","sizes":["48x48"],"src":"https://example.com/icon.png","theme":"dark"}"#;
        let deserialized: Icon = serde_json::from_str(json).unwrap();
        
        assert_eq!(deserialized, create_test_icon())  
    }
}
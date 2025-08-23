//! Utilities for Primitive JSON schema definitions

use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::types::PropertyType;

/// Represents restricted subset of JSON Schema: 
/// [`StringSchema`], [`NumberSchema`], [`BooleanSchema`], or [`EnumSchema`].
#[derive(Debug, Clone, Serialize, Deserialize)]
//#[serde(untagged)]
pub enum Schema {
    /// See [`StringSchema`]
    String(StringSchema),
    
    /// See [`NumberSchema`]
    Number(NumberSchema),
    
    /// See [`BooleanSchema`]
    Boolean(BooleanSchema),
    
    /// See [`EnumSchema`]   
    Enum(EnumSchema),
}

/// Represents a schema for a string type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StringSchema {
    /// Property type
    #[serde(rename = "type", default)]
    pub r#type: PropertyType,

    /// A title for the property.
    #[serde(rename = "title", skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A human-readable description of the property
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,
    
    /// The minimum length for the string.
    #[serde(rename = "minLength", skip_serializing_if = "Option::is_none")]
    pub min_length: Option<usize>,
    
    /// The maximum length for the string.
    #[serde(rename = "maxLength", skip_serializing_if = "Option::is_none")]
    pub max_length: Option<usize>,
    
    /// A specific format for the string ("email", "uri", "date", or "date-time").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

/// Represents a schema for a number type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NumberSchema {
    /// Property type
    #[serde(rename = "type", default)]
    pub r#type: PropertyType,

    /// A title for the property.
    #[serde(rename = "title", skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A human-readable description of the property
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,
    
    /// The minimum allowed value.
    #[serde(rename = "minimum", skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    
    /// The maximum allowed value.
    #[serde(rename = "maximum", skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

/// Represents a schema for a boolean type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BooleanSchema {
    /// Property type
    #[serde(rename = "type", default)]
    pub r#type: PropertyType,

    /// A title for the property.
    #[serde(rename = "title", skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A human-readable description of the property
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,
    
    /// The default value for the Boolean.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<bool>,
}

/// Represents a schema for an enum type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumSchema {
    /// Property type
    #[serde(rename = "type", default)]
    pub r#type: PropertyType,

    /// A title for the property.
    #[serde(rename = "title", skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A human-readable description of the property
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,
    
    /// The list of allowed string values for the enum.
    #[serde(rename = "enum")]
    pub r#enum: Vec<String>,
 
    /// Optional display names corresponding to the enum values
    #[serde(rename = "enumNames", skip_serializing_if = "Option::is_none")]
    pub enum_names: Option<Vec<String>>,   
}

impl Default for StringSchema {
    #[inline]
    fn default() -> Self {
        Self {
            r#type: PropertyType::String,
            title: None,
            descr: None,
            format: None,
            max_length: None,
            min_length: None,
        }
    }
}

impl Default for NumberSchema {
    fn default() -> Self {
        Self {
            r#type: PropertyType::Number,
            title: None,
            descr: None,
            max: None,
            min: None,
        }
    }
}

impl Default for BooleanSchema {
    #[inline]
    fn default() -> Self {
        Self {
            r#type: PropertyType::Bool,
            default: Some(false),
            title: None,
            descr: None,
        }
    }
}

impl Default for EnumSchema {
    #[inline]
    fn default() -> Self {
        Self {
            r#type: PropertyType::Object,
            r#enum: Vec::with_capacity(4),
            title: None,
            descr: None,
            enum_names: None
        }
    }
}

impl From<&str> for Schema {
    #[inline]
    fn from(value: &str) -> Self {
        match value { 
            "string" => Self::string(),
            "number" => Self::number(),
            "boolean" => Self::boolean(),
            "enum" => Self::enumeration(),
            _ => Self::string(),
        }
    }
}

impl From<&Value> for Schema {
    #[inline]
    fn from(value: &Value) -> Self {
        let type_str = value.get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("string");
        match type_str {
            "number" | "integer" => Schema::Number(NumberSchema::from(value)),
            "string" => Schema::String(StringSchema::from(value)),
            "boolean" => Schema::Boolean(BooleanSchema::from(value)),
            _ => Schema::Enum(EnumSchema::from(value)),       
        }
    }
}

impl From<&Value> for NumberSchema {
    #[inline]
    fn from(value: &Value) -> Self {
        Self {
            r#type: PropertyType::Number,
            title: value.get("title")
                .and_then(|v| v.as_str())
                .map(|v| v.to_string()),
            descr: value.get("description")
                .and_then(|v| v.as_str())
                .map(|v| v.to_string()),
            min: value.get("min")
                .and_then(|v| v.as_f64()),
            max: value.get("max")
                .and_then(|v| v.as_f64())
        }
    }
}

impl From<&Value> for StringSchema {
    #[inline]
    fn from(value: &Value) -> Self {
        Self {
            r#type: PropertyType::String,
            title: value.get("title")
                .and_then(|v| v.as_str())
                .map(|v| v.to_string()),
            descr: value.get("description")
                .and_then(|v| v.as_str())
                .map(|v| v.to_string()),
            format: value.get("format")
                .and_then(|v| v.as_str())
                .map(|v| v.to_string()),
            min_length: value.get("min_length")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize),
            max_length: value.get("max_length")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
        }
    }
}

impl From<&Value> for BooleanSchema {
    #[inline]
    fn from(value: &Value) -> Self {
        Self {
            r#type: PropertyType::Bool,
            title: value.get("title")
                .and_then(|v| v.as_str())
                .map(|v| v.to_string()),
            descr: value.get("description")
                .and_then(|v| v.as_str())
                .map(|v| v.to_string()),
            default: value.get("default")
                .and_then(|v| v.as_bool())
        }
    }
}

impl From<&Value> for EnumSchema {
    #[inline]
    fn from(value: &Value) -> Self {
        Self {
            r#type: PropertyType::Bool,
            title: value.get("title")
                .and_then(|v| v.as_str())
                .map(|v| v.to_string()),
            descr: value.get("description")
                .and_then(|v| v.as_str())
                .map(|v| v.to_string()),
            r#enum: Vec::new(),
            enum_names: None,
        }
    }
}

impl From<String> for Schema {
    #[inline]
    fn from(value: String) -> Self {
        value.as_str().into()
    }
}

impl Schema {
    /// Creates a new [`Schema`] instance with a [`StringSchema`] type.
    pub fn string() -> Self {
        Self::String(StringSchema::default())
    }
    
    /// Creates a new [`Schema`] instance with a [`NumberSchema`] type.
    pub fn number() -> Self {
        Self::Number(NumberSchema::default())
    }
    
    /// Creates a new [`Schema`] instance with a [`BooleanSchema`] type.   
    pub fn boolean() -> Self {
        Self::Boolean(BooleanSchema::default())
    }
    
    /// Creates a new [`Schema`] instance with a [`EnumSchema`] type.  
    pub fn enumeration() -> Self {
        Self::Enum(EnumSchema::default())   
    }
}

impl StringSchema {
    
}
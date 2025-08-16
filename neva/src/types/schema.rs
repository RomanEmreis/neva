//! Utilities for Primitive JSON schema definitions

use serde::{Deserialize, Serialize};
use crate::types::PropertyType;

/// Represents restricted subset of JSON Schema: 
/// [`StringSchema`], [`NumberSchema`], [`BooleanSchema`], or [`EnumSchema`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
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
 
    /// ptional display names corresponding to the enum values
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
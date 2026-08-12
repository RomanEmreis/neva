//! Utilities for Primitive JSON schema definitions

use crate::prelude::Schema::{MultiTitledEnum, SingleTitledEnum};
use crate::types::Schema::{MultiUntitledEnum, SingleUntitledEnum};
use crate::{
    error::{Error, ErrorCode},
    types::PropertyType,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// Represents restricted subset of JSON Schema:
/// - [`StringSchema`]
/// - [`NumberSchema`]
/// - [`BooleanSchema`]
/// - [`UntitledSingleSelectEnumSchema`]
/// - [`TitledSingleSelectEnumSchema`]
/// - [`UntitledMultiSelectEnumSchema`]
/// - [`TitledMultiSelectEnumSchema`]
#[derive(Debug, Clone)]
pub enum Schema {
    /// See [`StringSchema`]
    String(StringSchema),

    /// See [`NumberSchema`]
    Number(NumberSchema),

    /// See [`BooleanSchema`]
    Boolean(BooleanSchema),

    /// See [`UntitledSingleSelectEnumSchema`]
    SingleUntitledEnum(UntitledSingleSelectEnumSchema),

    /// See [`TitledSingleSelectEnumSchema`]
    SingleTitledEnum(TitledSingleSelectEnumSchema),

    /// See [`UntitledMultiSelectEnumSchema`]
    MultiUntitledEnum(UntitledMultiSelectEnumSchema),

    /// See [`TitledMultiSelectEnumSchema`]
    MultiTitledEnum(TitledMultiSelectEnumSchema),

    /// See [`LegacyTitledEnumSchema`]
    LegacyEnum(LegacyTitledEnumSchema),
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
    pub format: Option<StringFormat>,

    /// Every keyword this type does not model, kept verbatim.
    ///
    /// A schema is a document, not a fixed set of fields: `default`,
    /// `examples`, `pattern`, `$comment` and anything a vocabulary adds are all
    /// meaningful to the peer that reads them. Modelling the ones neva acts on
    /// and dropping the rest would make the schema a peer receives quietly
    /// different from the one the server declared -- so the rest round-trips
    /// through here instead.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, Value>,
}

/// Represents a specific format for the string ("email", "uri", "date", or "date-time").
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum StringFormat {
    /// Email format.
    #[serde(rename = "email")]
    Email,

    /// URI format.
    #[serde(rename = "uri")]
    Uri,

    /// Date format.
    #[serde(rename = "date")]
    Date,

    /// Date-time format.
    #[serde(rename = "date-time")]
    DateTime,
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

    /// Every keyword this type does not model, kept verbatim.
    ///
    /// A schema is a document, not a fixed set of fields: `default`,
    /// `examples`, `pattern`, `$comment` and anything a vocabulary adds are all
    /// meaningful to the peer that reads them. Modelling the ones neva acts on
    /// and dropping the rest would make the schema a peer receives quietly
    /// different from the one the server declared -- so the rest round-trips
    /// through here instead.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, Value>,
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

    /// Every keyword this type does not model, kept verbatim.
    ///
    /// A schema is a document, not a fixed set of fields: `default`,
    /// `examples`, `pattern`, `$comment` and anything a vocabulary adds are all
    /// meaningful to the peer that reads them. Modelling the ones neva acts on
    /// and dropping the rest would make the schema a peer receives quietly
    /// different from the one the server declared -- so the rest round-trips
    /// through here instead.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, Value>,
}

/// Legacy enumeration schema for the protocol versions below `2025-11-25`.
/// For the newer versions use the [`TitledSingleSelectEnumSchema`] instead.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyTitledEnumSchema {
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

    /// Every keyword this type does not model, kept verbatim.
    ///
    /// A schema is a document, not a fixed set of fields: `default`,
    /// `examples`, `pattern`, `$comment` and anything a vocabulary adds are all
    /// meaningful to the peer that reads them. Modelling the ones neva acts on
    /// and dropping the rest would make the schema a peer receives quietly
    /// different from the one the server declared -- so the rest round-trips
    /// through here instead.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, Value>,
}

/// Schema for single-selection enumeration without display titles for options.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UntitledSingleSelectEnumSchema {
    /// Property type
    #[serde(rename = "type", default)]
    pub r#type: PropertyType,

    /// A title for the property.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A human-readable description of the property
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,

    /// The list of allowed string values for the enum.
    #[serde(rename = "enum")]
    pub r#enum: Vec<String>,

    /// Optional default value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,

    /// Every keyword this type does not model, kept verbatim.
    ///
    /// A schema is a document, not a fixed set of fields: `default`,
    /// `examples`, `pattern`, `$comment` and anything a vocabulary adds are all
    /// meaningful to the peer that reads them. Modelling the ones neva acts on
    /// and dropping the rest would make the schema a peer receives quietly
    /// different from the one the server declared -- so the rest round-trips
    /// through here instead.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, Value>,
}

/// Schema for single-selection enumeration with display titles for each option.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TitledSingleSelectEnumSchema {
    /// Property type
    #[serde(rename = "type", default)]
    pub r#type: PropertyType,

    /// A title for the property.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A human-readable description of the property
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,

    /// The list of enum options with values and display labels.
    #[serde(rename = "oneOf")]
    pub one_of: Vec<EnumOption>,

    /// Optional default value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,

    /// Every keyword this type does not model, kept verbatim.
    ///
    /// A schema is a document, not a fixed set of fields: `default`,
    /// `examples`, `pattern`, `$comment` and anything a vocabulary adds are all
    /// meaningful to the peer that reads them. Modelling the ones neva acts on
    /// and dropping the rest would make the schema a peer receives quietly
    /// different from the one the server declared -- so the rest round-trips
    /// through here instead.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, Value>,
}

/// Schema for multiple-selection enumeration without display titles for options.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UntitledMultiSelectEnumSchema {
    /// Property type
    #[serde(rename = "type", default)]
    pub r#type: PropertyType,

    /// A title for the property.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A human-readable description of the property
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,

    /// The list of allowed string values for the enum.
    pub items: EnumItems,

    /// Maximum number of items to select.
    #[serde(rename = "maxItems", skip_serializing_if = "Option::is_none")]
    pub max_items: Option<usize>,

    /// Minimum number of items to select.
    #[serde(rename = "minItems", skip_serializing_if = "Option::is_none")]
    pub min_items: Option<usize>,

    /// Optional default value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Vec<String>>,

    /// Every keyword this type does not model, kept verbatim.
    ///
    /// A schema is a document, not a fixed set of fields: `default`,
    /// `examples`, `pattern`, `$comment` and anything a vocabulary adds are all
    /// meaningful to the peer that reads them. Modelling the ones neva acts on
    /// and dropping the rest would make the schema a peer receives quietly
    /// different from the one the server declared -- so the rest round-trips
    /// through here instead.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, Value>,
}

/// Schema for multiple-selection enumeration with display titles for each option.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TitledMultiSelectEnumSchema {
    /// Property type
    #[serde(rename = "type", default)]
    pub r#type: PropertyType,

    /// A title for the property.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// A human-readable description of the property
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,

    /// The list of allowed string values for the enum.
    pub items: EnumOptions,

    /// Maximum number of items to select.
    #[serde(rename = "maxItems", skip_serializing_if = "Option::is_none")]
    pub max_items: Option<usize>,

    /// Minimum number of items to select.
    #[serde(rename = "minItems", skip_serializing_if = "Option::is_none")]
    pub min_items: Option<usize>,

    /// Optional default value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Vec<String>>,

    /// Every keyword this type does not model, kept verbatim.
    ///
    /// A schema is a document, not a fixed set of fields: `default`,
    /// `examples`, `pattern`, `$comment` and anything a vocabulary adds are all
    /// meaningful to the peer that reads them. Modelling the ones neva acts on
    /// and dropping the rest would make the schema a peer receives quietly
    /// different from the one the server declared -- so the rest round-trips
    /// through here instead.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, Value>,
}

/// Schema for the array of enumeration items.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumItems {
    /// Property type
    #[serde(rename = "type", default)]
    pub r#type: PropertyType,

    /// The list of allowed string values for the enum.
    #[serde(rename = "enum")]
    pub r#enum: Vec<String>,

    /// Every keyword this type does not model, kept verbatim -- see
    /// [`Schema`]'s own `extra`. A schema nests, so the round trip has to hold
    /// below the root as well: a `pattern` beside an `enum` is the server's
    /// constraint wherever it sits.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, Value>,
}

/// Schema for array items with enum options and display labels.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumOptions {
    /// Array of enum options with values and display labels.
    #[serde(rename = "anyOf")]
    pub any_of: Vec<EnumOption>,

    /// Every keyword this type does not model, kept verbatim -- see
    /// [`Schema`]'s own `extra`. A schema nests, so the round trip has to hold
    /// below the root as well: a `pattern` beside an `enum` is the server's
    /// constraint wherever it sits.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, Value>,
}

/// Represents an enumeration option with a display title.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnumOption {
    /// The enum value.
    #[serde(rename = "const")]
    pub value: String,

    /// Display label for this option.
    pub title: String,

    /// Every keyword this type does not model, kept verbatim -- see
    /// [`Schema`]'s own `extra`. A schema nests, so the round trip has to hold
    /// below the root as well: a `pattern` beside an `enum` is the server's
    /// constraint wherever it sits.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, Value>,
}

impl<'de> Deserialize<'de> for Schema {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let obj = value
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("Expected object"))?;

        let type_field = obj.get("type").and_then(|v| v.as_str());
        let schema = match type_field {
            Some("string") => {
                if obj.contains_key("enum") {
                    if obj.contains_key("enumNames") {
                        Schema::LegacyEnum(
                            serde_json::from_value(value).map_err(serde::de::Error::custom)?,
                        )
                    } else {
                        Schema::SingleUntitledEnum(
                            serde_json::from_value(value).map_err(serde::de::Error::custom)?,
                        )
                    }
                } else if obj.contains_key("oneOf") {
                    Schema::SingleTitledEnum(
                        serde_json::from_value(value).map_err(serde::de::Error::custom)?,
                    )
                } else {
                    Schema::String(serde_json::from_value(value).map_err(serde::de::Error::custom)?)
                }
            }
            Some("array") => {
                let items = obj.get("items");
                if let Some(items_obj) = items.and_then(|v| v.as_object()) {
                    if items_obj.contains_key("anyOf") {
                        Schema::MultiTitledEnum(
                            serde_json::from_value(value).map_err(serde::de::Error::custom)?,
                        )
                    } else if items_obj.contains_key("enum") {
                        Schema::MultiUntitledEnum(
                            serde_json::from_value(value).map_err(serde::de::Error::custom)?,
                        )
                    } else {
                        return Err(serde::de::Error::custom("Unknown array schema type"));
                    }
                } else {
                    return Err(serde::de::Error::custom("Array schema missing items"));
                }
            }
            Some("number") | Some("integer") => {
                Schema::Number(serde_json::from_value(value).map_err(serde::de::Error::custom)?)
            }
            Some("boolean") => {
                Schema::Boolean(serde_json::from_value(value).map_err(serde::de::Error::custom)?)
            }
            _ => {
                return Err(serde::de::Error::custom(format!(
                    "Unknown or missing type field: {:?}",
                    type_field
                )));
            }
        };

        Ok(schema)
    }
}

impl Serialize for Schema {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Schema::String(s) => s.serialize(serializer),
            Schema::Number(n) => n.serialize(serializer),
            Schema::Boolean(b) => b.serialize(serializer),
            Schema::SingleUntitledEnum(e) => e.serialize(serializer),
            Schema::SingleTitledEnum(e) => e.serialize(serializer),
            Schema::MultiUntitledEnum(e) => e.serialize(serializer),
            Schema::MultiTitledEnum(e) => e.serialize(serializer),
            Schema::LegacyEnum(e) => e.serialize(serializer),
        }
    }
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
            extra: Default::default(),
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
            extra: Default::default(),
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
            extra: Default::default(),
        }
    }
}

impl Default for LegacyTitledEnumSchema {
    #[inline]
    fn default() -> Self {
        Self {
            r#type: PropertyType::String,
            r#enum: Vec::new(),
            title: None,
            descr: None,
            enum_names: None,
            extra: Default::default(),
        }
    }
}

impl Default for UntitledSingleSelectEnumSchema {
    #[inline]
    fn default() -> Self {
        Self {
            r#type: PropertyType::String,
            r#enum: Vec::new(),
            title: None,
            descr: None,
            default: None,
            extra: Default::default(),
        }
    }
}

impl Default for TitledSingleSelectEnumSchema {
    #[inline]
    fn default() -> Self {
        Self {
            r#type: PropertyType::String,
            one_of: Vec::new(),
            title: None,
            descr: None,
            default: None,
            extra: Default::default(),
        }
    }
}

impl Default for UntitledMultiSelectEnumSchema {
    #[inline]
    fn default() -> Self {
        Self {
            r#type: PropertyType::Array,
            items: Default::default(),
            max_items: None,
            min_items: None,
            title: None,
            descr: None,
            default: None,
            extra: Default::default(),
        }
    }
}

impl Default for TitledMultiSelectEnumSchema {
    #[inline]
    fn default() -> Self {
        Self {
            r#type: PropertyType::Array,
            items: Default::default(),
            max_items: None,
            min_items: None,
            title: None,
            descr: None,
            default: None,
            extra: Default::default(),
        }
    }
}

impl Default for EnumItems {
    #[inline]
    fn default() -> Self {
        Self {
            r#type: PropertyType::String,
            r#enum: Vec::new(),
            extra: Default::default(),
        }
    }
}

impl Default for EnumOptions {
    #[inline]
    fn default() -> Self {
        Self {
            any_of: Vec::new(),
            extra: Default::default(),
        }
    }
}

impl From<&str> for Schema {
    #[inline]
    fn from(value: &str) -> Self {
        match value {
            "string" => Self::string(),
            "number" => Self::number(),
            "integer" => Self::integer(),
            "boolean" => Self::boolean(),
            "enum" => Self::single_titled_enum(),
            "array" => Self::multi_titled_enum(),
            _ => Self::string(),
        }
    }
}

impl From<&Value> for Schema {
    #[inline]
    fn from(value: &Value) -> Self {
        serde_json::from_value(value.clone()).unwrap_or_else(|_| Schema::string())
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
        Self::String(Default::default())
    }

    /// Creates a new [`Schema`] instance with a [`NumberSchema`] type.
    pub fn number() -> Self {
        Self::Number(Default::default())
    }

    /// Creates a new [`Schema`] instance typed `integer`.
    ///
    /// Backed by [`NumberSchema`] -- the shape is the same -- but published as
    /// `"integer"`, which rejects `1.5` where `"number"` accepts it.
    ///
    /// # Examples
    /// ```
    /// use neva::types::Schema;
    ///
    /// let schema = serde_json::to_value(Schema::integer())?;
    /// assert_eq!(schema["type"], "integer");
    /// # Ok::<(), serde_json::Error>(())
    /// ```
    pub fn integer() -> Self {
        Self::Number(NumberSchema {
            r#type: PropertyType::Integer,
            ..Default::default()
        })
    }

    /// Creates a new [`Schema`] instance with a [`BooleanSchema`] type.   
    pub fn boolean() -> Self {
        Self::Boolean(Default::default())
    }

    /// Creates a new [`Schema`] instance with a [`SingleUntitledEnum`] type.  
    pub fn single_untitled_enum() -> Self {
        SingleUntitledEnum(Default::default())
    }

    /// Creates a new [`Schema`] instance with a [`SingleTitledEnum`] type.  
    pub fn single_titled_enum() -> Self {
        SingleTitledEnum(Default::default())
    }

    /// Creates a new [`Schema`] instance with a [`MultiUntitledEnum`] type.  
    pub fn multi_untitled_enum() -> Self {
        MultiUntitledEnum(Default::default())
    }

    /// Creates a new [`Schema`] instance with a [`MultiTitledEnum`] type.  
    pub fn multi_titled_enum() -> Self {
        MultiTitledEnum(Default::default())
    }
}

impl StringSchema {
    /// Validates the string value against the schema.
    #[inline]
    pub(crate) fn validate(&self, value: &Value) -> Result<(), Error> {
        let str_value = value.as_str().ok_or(Error::new(
            ErrorCode::InvalidParams,
            "Expected string value",
        ))?;

        if let Some(min_len) = self.min_length
            && str_value.len() < min_len
        {
            return Err(Error::new(
                ErrorCode::InvalidParams,
                format!("String too short: {} < {min_len}", str_value.len()),
            ));
        }

        if let Some(max_len) = self.max_length
            && str_value.len() > max_len
        {
            return Err(Error::new(
                ErrorCode::InvalidParams,
                format!("String too long: {} > {max_len}", str_value.len()),
            ));
        }

        // Validate format if specified
        if let Some(format) = &self.format {
            self.validate_string_format(str_value, format)?;
        }

        Ok(())
    }

    /// Validates a string format (basic validation for common formats)
    fn validate_string_format(&self, value: &str, format: &StringFormat) -> Result<(), Error> {
        match format {
            StringFormat::Email => {
                if !value.contains('@') || !value.contains('.') {
                    return Err(Error::new(ErrorCode::InvalidParams, "Invalid email format"));
                }
            }
            StringFormat::Uri => {
                let parts: Vec<&str> = value.splitn(2, "://").collect();
                if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
                    return Err(Error::new(ErrorCode::InvalidParams, "Invalid URI format"));
                }
            }
            StringFormat::Date => {
                // Basic date format validation (YYYY-MM-DD)
                if value.len() != 10
                    || value.chars().nth(4) != Some('-')
                    || value.chars().nth(7) != Some('-')
                {
                    return Err(Error::new(
                        ErrorCode::InvalidParams,
                        "Invalid date format (expected YYYY-MM-DD)",
                    ));
                }
            }
            StringFormat::DateTime => {
                if !value.contains('T') {
                    return Err(Error::new(ErrorCode::InvalidParams, "Invalid date format"));
                }
            }
        }
        Ok(())
    }
}

impl NumberSchema {
    /// Validates the number value against the schema.
    #[inline]
    pub(crate) fn validate(&self, value: &Value) -> Result<(), Error> {
        let num_value = value.as_f64().ok_or(Error::new(
            ErrorCode::InvalidParams,
            "Expected number value",
        ))?;

        // `integer` and `number` share this struct because their keywords are
        // the same, but they are different JSON Schema types and the peer was
        // told which one applies. Checking the fractional part rather than the
        // JSON representation is deliberate: `1.0` is a valid integer under
        // 2020-12, and a client that sends it is not in the wrong.
        if self.r#type == PropertyType::Integer && num_value.fract() != 0.0 {
            return Err(Error::new(
                ErrorCode::InvalidParams,
                format!("Expected integer value, got: {num_value}"),
            ));
        }

        if let Some(min) = self.min
            && num_value < min
        {
            return Err(Error::new(
                ErrorCode::InvalidParams,
                format!("Number too small: {num_value} < {min}"),
            ));
        }

        if let Some(max) = self.max
            && num_value > max
        {
            return Err(Error::new(
                ErrorCode::InvalidParams,
                format!("Number too large: {num_value} > {max}"),
            ));
        }

        Ok(())
    }
}

impl BooleanSchema {
    /// Validates the boolean value against the schema.
    #[inline]
    pub(crate) fn validate(&self, value: &Value) -> Result<(), Error> {
        value
            .is_boolean()
            .then_some(())
            .ok_or_else(|| Error::new(ErrorCode::InvalidParams, "Expected boolean value"))
    }
}

impl LegacyTitledEnumSchema {
    /// Validates the legacy enumeration value against the schema.
    #[inline]
    pub(crate) fn validate(&self, value: &Value) -> Result<(), Error> {
        let str_value = make_str(value)?;
        self.r#enum
            .iter()
            .any(|v| v == str_value)
            .then_some(())
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::InvalidParams,
                    format!("Invalid enum value: {str_value}"),
                )
            })
    }
}

impl UntitledSingleSelectEnumSchema {
    /// Validates the enumeration value against the schema.
    #[inline]
    pub(crate) fn validate(&self, value: &Value) -> Result<(), Error> {
        let str_value = make_str(value)?;
        self.r#enum
            .iter()
            .any(|v| v == str_value)
            .then_some(())
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::InvalidParams,
                    format!("Invalid enum value: {str_value}"),
                )
            })
    }
}

impl TitledSingleSelectEnumSchema {
    /// Validates the enumeration value against the schema.
    #[inline]
    pub(crate) fn validate(&self, value: &Value) -> Result<(), Error> {
        let str_value = make_str(value)?;
        self.one_of
            .iter()
            .any(|v| v.value == str_value)
            .then_some(())
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::InvalidParams,
                    format!("Invalid enum value: {str_value}"),
                )
            })
    }
}

impl UntitledMultiSelectEnumSchema {
    /// Validates the enumeration value against the schema.
    #[inline]
    pub(crate) fn validate(&self, value: &Value) -> Result<(), Error> {
        let mut str_values = make_iter_of_as_array(value)?;
        str_values
            .all(|value| self.items.r#enum.iter().any(|v| v == value))
            .then_some(())
            .ok_or_else(|| Error::new(ErrorCode::InvalidParams, "Invalid enum values"))
    }
}

impl TitledMultiSelectEnumSchema {
    /// Validates the enumeration value against the schema.
    #[inline]
    pub(crate) fn validate(&self, value: &Value) -> Result<(), Error> {
        let mut str_values = make_iter_of_as_array(value)?;
        str_values
            .all(|value| self.items.any_of.iter().any(|v| v.value == value))
            .then_some(())
            .ok_or_else(|| Error::new(ErrorCode::InvalidParams, "Invalid enum values"))
    }
}

impl EnumOptions {
    /// Creates a new [`EnumOptions`]
    #[inline]
    pub fn new(options: impl IntoIterator<Item = EnumOption>) -> Self {
        Self {
            any_of: options.into_iter().collect(),
            extra: Default::default(),
        }
    }
}

impl EnumOption {
    /// Creates a new [`EnumOption`]
    #[inline]
    pub fn new<S: Into<String>>(value: S, title: S) -> Self {
        Self {
            value: value.into(),
            title: title.into(),
            extra: Default::default(),
        }
    }
}

impl EnumItems {
    /// Create a new [`EnumItems`]
    pub fn new<I, S>(items: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            r#type: PropertyType::String,
            r#enum: items.into_iter().map(|s| s.into()).collect(),
            extra: Default::default(),
        }
    }
}

#[inline(always)]
fn make_iter_of_as_array(value: &Value) -> Result<impl Iterator<Item = &str>, Error> {
    value
        .as_array()
        .map(|v| v.iter().filter_map(|v| v.as_str()))
        .ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidParams,
                "Expected an array of values for enum",
            )
        })
}

#[inline(always)]
fn make_str(value: &Value) -> Result<&str, Error> {
    value
        .as_str()
        .ok_or_else(|| Error::new(ErrorCode::InvalidParams, "Expected string value for enum"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn it_validates_string_length() {
        let schema = StringSchema {
            min_length: Some(3),
            max_length: Some(5),
            ..Default::default()
        };

        assert!(schema.validate(&json!("abc")).is_ok());
        assert!(schema.validate(&json!("ab")).is_err());
        assert!(schema.validate(&json!("abcdef")).is_err());
    }

    #[test]
    fn it_validates_string_format_email() {
        let schema = StringSchema {
            format: Some(StringFormat::Email),
            ..Default::default()
        };

        assert!(schema.validate(&json!("test@example.com")).is_ok());
        assert!(schema.validate(&json!("invalid")).is_err());
        assert!(schema.validate(&json!("no.at.symbol")).is_err());
        assert!(schema.validate(&json!("no_dot@domain")).is_err());
    }

    #[test]
    fn it_validates_string_format_uri() {
        let schema = StringSchema {
            format: Some(StringFormat::Uri),
            ..Default::default()
        };

        assert!(schema.validate(&json!("https://example.com")).is_ok());
        assert!(schema.validate(&json!("invalid")).is_err());
        assert!(schema.validate(&json!("://empty")).is_err());
    }

    #[test]
    fn it_validates_string_format_date() {
        let schema = StringSchema {
            format: Some(StringFormat::Date),
            ..Default::default()
        };

        assert!(schema.validate(&json!("2023-01-01")).is_ok());
        assert!(schema.validate(&json!("2023/01/01")).is_err());
        assert!(schema.validate(&json!("2023-1-1")).is_err());
    }

    #[test]
    fn it_validates_string_format_date_time() {
        let schema = StringSchema {
            format: Some(StringFormat::DateTime),
            ..Default::default()
        };

        assert!(schema.validate(&json!("2023-01-01T12:00:00Z")).is_ok());
        assert!(schema.validate(&json!("2023-01-01 12:00:00")).is_err());
    }

    #[test]
    fn it_validates_number_range() {
        let schema = NumberSchema {
            min: Some(10.0),
            max: Some(20.0),
            ..Default::default()
        };

        assert!(schema.validate(&json!(15)).is_ok());
        assert!(schema.validate(&json!(10)).is_ok());
        assert!(schema.validate(&json!(20)).is_ok());
        assert!(schema.validate(&json!(9)).is_err());
        assert!(schema.validate(&json!(21)).is_err());
    }

    /// A field published as `"integer"` must be held to it: the peer was told
    /// `1.5` is not a legal value there, and validating it as a plain number
    /// would accept exactly what the declared schema rules out.
    #[test]
    fn an_integer_field_is_held_to_whole_values() {
        let Schema::Number(schema) = Schema::integer() else {
            panic!("Schema::integer must be backed by NumberSchema");
        };

        assert!(schema.validate(&json!(1)).is_ok());
        assert!(schema.validate(&json!(-7)).is_ok());
        // 2020-12 judges the value, not how it was written: a zero fractional
        // part is an integer.
        assert!(schema.validate(&json!(1.0)).is_ok());
        assert!(schema.validate(&json!(1.5)).is_err());

        // ...and `number` still accepts what `integer` refuses.
        assert!(NumberSchema::default().validate(&json!(1.5)).is_ok());
    }

    /// The integer check runs before the bounds, but must not shadow them:
    /// both refusals stay reachable.
    #[test]
    fn an_integer_field_still_carries_its_bounds() {
        let schema = NumberSchema {
            r#type: PropertyType::Integer,
            min: Some(1.0),
            max: Some(10.0),
            ..Default::default()
        };

        assert!(schema.validate(&json!(5)).is_ok());
        assert!(schema.validate(&json!(11)).is_err(), "above the maximum");
        assert!(schema.validate(&json!(5.5)).is_err(), "not an integer");
    }

    #[test]
    fn it_validates_boolean() {
        let schema = BooleanSchema::default();
        assert!(schema.validate(&json!(true)).is_ok());
        assert!(schema.validate(&json!(false)).is_ok());
        assert!(schema.validate(&json!("true")).is_err());
    }

    #[test]
    fn it_validates_legacy_enum() {
        let schema = LegacyTitledEnumSchema {
            r#enum: vec!["A".to_string(), "B".to_string()],
            ..Default::default()
        };

        assert!(schema.validate(&json!("A")).is_ok());
        assert!(schema.validate(&json!("C")).is_err());
    }

    #[test]
    fn it_validates_untitled_single_select_enum() {
        let schema = UntitledSingleSelectEnumSchema {
            r#enum: vec!["A".to_string(), "B".to_string()],
            ..Default::default()
        };

        assert!(schema.validate(&json!("A")).is_ok());
        assert!(schema.validate(&json!("C")).is_err());
    }

    #[test]
    fn it_validates_titled_single_select_enum() {
        let schema = TitledSingleSelectEnumSchema {
            one_of: vec![EnumOption::new("A", "A"), EnumOption::new("B", "B")],
            ..Default::default()
        };

        assert!(schema.validate(&json!("A")).is_ok());
        assert!(schema.validate(&json!("C")).is_err());
    }

    #[test]
    fn it_validates_untitled_multi_select_enum() {
        let schema = UntitledMultiSelectEnumSchema {
            items: EnumItems::new(["A", "B"]),
            ..Default::default()
        };

        assert!(schema.validate(&json!(["A", "B"])).is_ok());
        assert!(schema.validate(&json!(["A"])).is_ok());
        assert!(schema.validate(&json!(["C"])).is_err());
        assert!(schema.validate(&json!(["A", "C"])).is_err());
    }

    #[test]
    fn it_validates_titled_multi_select_enum() {
        let schema = TitledMultiSelectEnumSchema {
            items: EnumOptions::new([EnumOption::new("A", "A"), EnumOption::new("B", "B")]),
            ..Default::default()
        };

        assert!(schema.validate(&json!(["A", "B"])).is_ok());
        assert!(schema.validate(&json!(["A"])).is_ok());
        assert!(schema.validate(&json!(["C"])).is_err());
        assert!(schema.validate(&json!(["A", "C"])).is_err());
    }

    #[test]
    fn it_deserializes_schema_from_json() {
        let json = json!({
            "type": "string",
            "minLength": 5
        });
        let schema: Schema = serde_json::from_value(json).unwrap();
        if let Schema::String(s) = schema {
            assert_eq!(s.min_length, Some(5));
        } else {
            panic!("Expected StringSchema");
        }
    }

    #[test]
    fn it_serializes_schema_to_json() {
        let schema = Schema::String(StringSchema {
            min_length: Some(5),
            ..Default::default()
        });
        let json = serde_json::to_value(schema).unwrap();
        assert_eq!(
            json,
            json!({
                "type": "string",
                "minLength": 5
            })
        )
    }

    #[test]
    fn it_deserializes_number_schema_from_json() {
        let json = json!({
            "type": "number",
            "minimum": 5.0
        });
        let schema: Schema = serde_json::from_value(json).unwrap();
        if let Schema::Number(s) = schema {
            assert_eq!(s.min, Some(5.0));
        } else {
            panic!("Expected NumberSchema");
        }
    }

    #[test]
    fn it_serializes_number_schema_to_json() {
        let schema = Schema::Number(NumberSchema {
            min: Some(5.0),
            ..Default::default()
        });
        let json = serde_json::to_value(schema).unwrap();
        assert_eq!(
            json,
            json!({
                "type": "number",
                "minimum": 5.0
            })
        )
    }

    #[test]
    fn it_deserializes_boolean_schema_from_json() {
        let json = json!({
            "type": "boolean",
            "default": false
        });
        let schema: Schema = serde_json::from_value(json).unwrap();
        if let Schema::Boolean(s) = schema {
            assert_eq!(s.default, Some(false));
        } else {
            panic!("Expected BooleanSchema");
        }
    }

    #[test]
    fn it_serializes_boolean_schema_to_json() {
        let schema = Schema::Boolean(BooleanSchema {
            default: Some(false),
            ..Default::default()
        });
        let json = serde_json::to_value(schema).unwrap();
        assert_eq!(
            json,
            json!({
                "type": "boolean",
                "default": false
            })
        )
    }

    #[test]
    fn it_deserializes_untitled_single_select_enum_schema_from_json() {
        let json = json!({
            "type": "string",
            "enum": ["Red", "Green", "Blue"],
            "default": "Red"
        });
        let schema: Schema = serde_json::from_value(json).unwrap();
        if let SingleUntitledEnum(s) = schema {
            assert_eq!(s.r#enum, ["Red", "Green", "Blue"]);
        } else {
            panic!("Expected SingleUntitledEnumSchema");
        }
    }

    #[test]
    fn it_serializes_untitled_single_select_enum_schema_to_json() {
        let schema = SingleUntitledEnum(UntitledSingleSelectEnumSchema {
            r#enum: vec!["Red".into(), "Green".into(), "Blue".into()],
            default: Some("Red".into()),
            ..Default::default()
        });
        let json = serde_json::to_value(schema).unwrap();
        assert_eq!(
            json,
            json!({
                "type": "string",
                "enum": ["Red", "Green", "Blue"],
                "default": "Red"
            })
        )
    }

    #[test]
    fn it_deserializes_titled_single_select_enum_schema_from_json() {
        let json = json!({
            "type": "string",
            "oneOf": [
                { "const": "#FF0000", "title": "Red" },
                { "const": "#00FF00", "title": "Green" },
                { "const": "#0000FF", "title": "Blue" }
            ],
            "default": "#FF0000"
        });
        let schema: Schema = serde_json::from_value(json).unwrap();
        if let SingleTitledEnum(s) = schema {
            assert_eq!(
                s.one_of,
                [
                    EnumOption::new("#FF0000", "Red"),
                    EnumOption::new("#00FF00", "Green"),
                    EnumOption::new("#0000FF", "Blue"),
                ]
            );
        } else {
            panic!("Expected SingleTitledEnum");
        }
    }

    #[test]
    fn it_serializes_titled_single_select_enum_schema_to_json() {
        let schema = SingleTitledEnum(TitledSingleSelectEnumSchema {
            one_of: vec![
                EnumOption::new("#FF0000", "Red"),
                EnumOption::new("#00FF00", "Green"),
                EnumOption::new("#0000FF", "Blue"),
            ],
            default: Some("#FF0000".into()),
            ..Default::default()
        });
        let json = serde_json::to_value(schema).unwrap();
        assert_eq!(
            json,
            json!({
                "type": "string",
                "oneOf": [
                    { "const": "#FF0000", "title": "Red" },
                    { "const": "#00FF00", "title": "Green" },
                    { "const": "#0000FF", "title": "Blue" }
                ],
                "default": "#FF0000"
            })
        )
    }

    #[test]
    fn it_deserializes_titled_multi_select_enum_schema_from_json() {
        let json = json!({
            "type": "array",
            "items": {
                "anyOf": [
                    { "const": "#FF0000", "title": "Red" },
                    { "const": "#00FF00", "title": "Green" },
                    { "const": "#0000FF", "title": "Blue" }
                ]
            },
            "default": ["#FF0000", "#00FF00"]
        });
        let schema: Schema = serde_json::from_value(json).unwrap();
        if let MultiTitledEnum(s) = schema {
            assert_eq!(
                s.items.any_of,
                [
                    EnumOption::new("#FF0000", "Red"),
                    EnumOption::new("#00FF00", "Green"),
                    EnumOption::new("#0000FF", "Blue"),
                ]
            );
        } else {
            panic!("Expected MultiTitledEnum");
        }
    }

    #[test]
    fn it_serializes_titled_multi_select_enum_schema_to_json() {
        let schema = MultiTitledEnum(TitledMultiSelectEnumSchema {
            items: EnumOptions::new([
                EnumOption::new("#FF0000", "Red"),
                EnumOption::new("#00FF00", "Green"),
                EnumOption::new("#0000FF", "Blue"),
            ]),
            default: Some(vec!["#FF0000".into(), "#00FF00".into()]),
            ..Default::default()
        });
        let json = serde_json::to_value(schema).unwrap();
        assert_eq!(
            json,
            json!({
                "type": "array",
                "items": {
                    "anyOf": [
                        { "const": "#FF0000", "title": "Red" },
                        { "const": "#00FF00", "title": "Green" },
                        { "const": "#0000FF", "title": "Blue" }
                    ]
                },
                "default": ["#FF0000", "#00FF00"]
            })
        )
    }

    #[test]
    fn it_deserializes_untitled_multi_select_enum_schema_from_json() {
        let json = json!({
            "type": "array",
            "items": {
                "type": "string",
                "enum": ["Red", "Green", "Blue"]
            },
            "default": ["Red", "Green"]
        });
        let schema: Schema = serde_json::from_value(json).unwrap();
        if let MultiUntitledEnum(s) = schema {
            assert_eq!(s.items.r#enum, ["Red", "Green", "Blue"]);
        } else {
            panic!("Expected MultiTitledEnum");
        }
    }

    #[test]
    fn it_serializes_untitled_multi_select_enum_schema_to_json() {
        let schema = MultiUntitledEnum(UntitledMultiSelectEnumSchema {
            items: EnumItems::new(["Red", "Green", "Blue"]),
            default: Some(vec!["Red".into(), "Green".into()]),
            ..Default::default()
        });
        let json = serde_json::to_value(schema).unwrap();
        assert_eq!(
            json,
            json!({
                "type": "array",
                "items": {
                    "type": "string",
                    "enum": ["Red", "Green", "Blue"]
                },
                "default": ["Red", "Green"]
            })
        )
    }

    /// A schema is a document: the keywords neva does not model are the peer's
    /// business, and must come out the way they went in.
    #[test]
    fn unmodelled_keywords_survive_a_round_trip() {
        let declared = json!({
            "type": "string",
            "description": "a name",
            "default": "John Doe",
            "pattern": "^[A-Z]",
            "$comment": "kept verbatim"
        });

        let schema: Schema = serde_json::from_value(declared.clone()).expect("parses");
        assert_eq!(
            serde_json::to_value(&schema).expect("serializes"),
            declared,
            "every declared keyword must survive"
        );
    }

    /// A schema nests, so the round trip has to hold below the root.
    ///
    /// The enum forms put a schema inside `items` (and another inside each
    /// `anyOf` option), and a keyword sitting there is the peer's constraint
    /// just as much as one at the top. Keeping only the root's would publish an
    /// array of anything-shaped strings where the server declared a pattern.
    #[test]
    fn unmodelled_keywords_survive_inside_the_enum_forms() {
        for declared in [
            json!({
                "type": "array",
                "items": {
                    "type": "string",
                    "enum": ["x"],
                    "pattern": "^x",
                    "$comment": "inside items"
                }
            }),
            json!({
                "type": "array",
                "items": {
                    "anyOf": [
                        { "const": "x", "title": "Ex", "description": "the x one" }
                    ],
                    "$comment": "inside anyOf"
                }
            }),
        ] {
            let schema: Schema = serde_json::from_value(declared.clone())
                .unwrap_or_else(|e| panic!("{declared} must parse: {e}"));
            assert_eq!(
                serde_json::to_value(&schema).expect("serializes"),
                declared,
                "a keyword below the root must survive too"
            );
        }
    }

    /// `default` is the one SEP-1034 is about, and it is dropped by a type that
    /// only models what it validates.
    #[test]
    fn defaults_survive_on_every_primitive() {
        for declared in [
            json!({ "type": "string", "default": "John Doe" }),
            json!({ "type": "integer", "default": 30 }),
            json!({ "type": "number", "default": 95.5 }),
            json!({ "type": "boolean", "default": true }),
            json!({ "type": "string", "enum": ["active", "inactive"], "default": "active" }),
        ] {
            let schema: Schema = serde_json::from_value(declared.clone())
                .unwrap_or_else(|e| panic!("{declared} must parse: {e}"));
            assert_eq!(serde_json::to_value(&schema).expect("serializes"), declared);
        }
    }

    /// `integer` and `number` are different types in JSON Schema -- the first
    /// rejects `1.5`. They shared a variant, so a declared `integer` was
    /// published as the wider `number`.
    #[test]
    fn an_integer_stays_an_integer() {
        let schema: Schema = serde_json::from_value(json!({ "type": "integer" })).expect("parses");
        assert_eq!(
            serde_json::to_value(&schema).expect("serializes")["type"],
            "integer"
        );

        // ...and the two are still distinct when built rather than parsed.
        assert_eq!(
            serde_json::to_value(Schema::integer()).expect("serializes")["type"],
            "integer"
        );
        assert_eq!(
            serde_json::to_value(Schema::number()).expect("serializes")["type"],
            "number"
        );
        assert_eq!(
            serde_json::to_value(Schema::from("integer")).expect("serializes")["type"],
            "integer"
        );
    }
}

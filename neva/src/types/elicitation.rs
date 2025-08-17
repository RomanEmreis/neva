//! Utilities for Elicitation

#[cfg(feature = "client")]
use std::{future::Future, pin::Pin, sync::Arc};
use std::collections::HashMap;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use schemars::JsonSchema;
use crate::{
    types::{IntoResponse, PropertyType, RequestId, Response, Schema},
    //error::Error,
};


/// List of commands for Elicitation
pub mod commands {
    pub const CREATE: &str = "elicitation/create";
}

/// Represents a message issued from the server to elicit additional information from the user via the client.
#[derive(Serialize, Deserialize)]
pub struct ElicitRequestParams {
    /// The message to present to the user.
    pub message: String,
    
    /// The requested schema.
    /// 
    /// > **Note:** A restricted subset of JSON Schema.
    /// > Only top-level properties are allowed, without nesting.
    #[serde(rename = "requestedSchema")]
    pub schema: RequestSchema,
}

#[derive(Serialize, Deserialize)]
pub struct RequestSchema {
    /// The type of the schema.
    /// 
    /// > **Note:** always "object".
    #[serde(rename = "type", default)]
    pub r#type: PropertyType,
    
    /// The properties of the schema.
    pub properties: HashMap<String, Schema>,

    /// The required properties of the schema
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,
}

/// Represents the client's response to an elicitation request.
#[derive(Serialize, Deserialize)]
pub struct ElicitResult {
    /// The user action in response to the elicitation.
    /// 
    /// * "accept" - User submitted the form/confirmed the action.
    /// * "cancel" - User dismissed without making an explicit choice.
    /// * "decline" - User explicitly declined the action.
    pub action: ElicitationAction,
    
    /// The submitted form data.
    /// 
    /// > **Note:** This is typically omitted if the action is "cancel" or "decline".
    pub content: Option<Value>,
}

/// Represents the user's action in response to an elicitation request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ElicitationAction {
    /// User submitted the form/confirmed the action
    Accept,
    
    /// User dismissed without making an explicit choice
    Cancel,
    
    /// User explicitly declined the action
    Decline
}

impl Default for RequestSchema {
    #[inline]
    fn default() -> Self {
        Self {
            r#type: PropertyType::Object,
            properties: HashMap::with_capacity(8),
            required: None,
        }
    }
}

impl ElicitRequestParams {
    /// Creates a new [`ElicitRequestParams`]
    #[inline]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            schema: RequestSchema::new(),
        }
    }

    /// Adds a single optional property to the schema
    #[inline]
    pub fn with_prop(mut self, prop: &str, schema: impl Into<Schema>) -> Self {
        self.schema = self.schema
            .with_prop(prop, schema);
        self
    }
    
    /// Adds a single required property to the schema
    #[inline]
    pub fn with_required(mut self, prop: &str, schema: impl Into<Schema>) -> Self {
        self.schema = self.schema
            .with_required(prop, schema);
        self
    }

    /// Adds a schema from a type that implements [`Default`] and [`Serialize`]
    #[inline]
    pub fn with_schema<T: JsonSchema>(mut self) -> Self {
        self.schema = RequestSchema::of::<T>();
        self
    }
    
    pub fn validate_schema<T: Serialize + JsonSchema>(&self, content: T) -> ElicitResult {
        let source_schema = schemars::schema_for!(T);
        if let Err(validation_error) = self.validate_schema_compatibility(&source_schema) {
            return ElicitResult::decline().with_content(validation_error);
        }
        let content_value = match serde_json::to_value(&content) {
            Ok(value) => value,
            Err(err) => return ElicitResult::decline().with_content(err.to_string()),
        };
        if let Err(err) = self.validate_content_constraints(&content_value) {
            return ElicitResult::decline().with_content(err.to_string());
        }
        ElicitResult::accept().with_content(content)
    }

    /// Validates that the source schema is compatible with the target schema
    fn validate_schema_compatibility(&self, source: &schemars::Schema) -> Result<(), String> {
        let target = &self.schema;
        let source_props = source
            .get("properties")
            .and_then(|v| v.as_object())
            .ok_or("Source schema missing properties")?;

        let source_required = source
            .get("required")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
            .unwrap_or_default();

        // Check if all target properties exist in a source
        for prop_name in target.properties.keys() {
            if !source_props.contains_key(prop_name) {
                return Err(format!("Missing property: {prop_name}"));
            }
        }

        // Check if all required properties in the target are present
        if let Some(target_required) = &target.required {
            for required_prop in target_required {
                if !source_required.contains(&required_prop.as_str()) {
                    return Err(format!("Required property not marked as required: {required_prop}"));
                }
            }
        }

        Ok(())
    }

    /// Validates content against schema constraints
    fn validate_content_constraints(&self, content: &Value) -> Result<(), String> {
        let schema = &self.schema;
        let content_obj = content
            .as_object()
            .ok_or("Content is not an object")?;

        // Check required properties
        if let Some(required) = &schema.required {
            for required_prop in required {
                if !content_obj.contains_key(required_prop) {
                    return Err(format!("Missing required property: {}", required_prop));
                }
            }
        }

        // Validate each property against its schema
        for (prop_name, prop_schema) in &schema.properties {
            if let Some(prop_value) = content_obj.get(prop_name) {
                self.validate_property_value(prop_value, prop_schema)?;
            }
        }

        Ok(())
    }

    /// Validates a single property value against its schema
    fn validate_property_value(&self, value: &Value, schema: &Schema) -> Result<(), String> {
        match schema {
            Schema::String(string_schema) => {
                let str_value = value.as_str().ok_or("Expected string value")?;

                if let Some(min_len) = string_schema.min_length {
                    if str_value.len() < min_len {
                        return Err(format!("String too short: {} < {}", str_value.len(), min_len));
                    }
                }

                if let Some(max_len) = string_schema.max_length {
                    if str_value.len() > max_len {
                        return Err(format!("String too long: {} > {}", str_value.len(), max_len));
                    }
                }

                // Validate format if specified
                if let Some(format) = &string_schema.format {
                    self.validate_string_format(str_value, format)?;
                }
            },
            Schema::Number(number_schema) => {
                let num_value = value.as_f64().ok_or("Expected number value")?;

                if let Some(min) = number_schema.min {
                    if num_value < min {
                        return Err(format!("Number too small: {} < {}", num_value, min));
                    }
                }

                if let Some(max) = number_schema.max {
                    if num_value > max {
                        return Err(format!("Number too large: {} > {}", num_value, max));
                    }
                }
            },
            Schema::Boolean(_) => {
                if !value.is_boolean() {
                    return Err("Expected boolean value".to_string());
                }
            },
            Schema::Enum(enum_schema) => {
                let str_value = value.as_str().ok_or("Expected string value for enum")?;

                if !enum_schema.r#enum.contains(&str_value.to_string()) {
                    return Err(format!("Invalid enum value: {}", str_value));
                }
            },
        }

        Ok(())
    }

    /// Validates string format (basic validation for common formats)
    fn validate_string_format(&self, value: &str, format: &str) -> Result<(), String> {
        match format {
            "email" => {
                if !value.contains('@') || !value.contains('.') {
                    return Err("Invalid email format".to_string());
                }
            },
            "uri" => {
                if !value.starts_with("http://") && !value.starts_with("https://") && !value.starts_with("file://") {
                    return Err("Invalid URI format".to_string());
                }
            },
            "date" => {
                // Basic date format validation (YYYY-MM-DD)
                if value.len() != 10 || value.chars().nth(4) != Some('-') || value.chars().nth(7) != Some('-') {
                    return Err("Invalid date format (expected YYYY-MM-DD)".to_string());
                }
            },
            "date-time" => {
                // Basic datetime validation (contains 'T' separator)
                if !value.contains('T') {
                    return Err("Invalid date-time format".to_string());
                }
            },
            _ => {
                // Unknown format, skip validation
            }
        }
        Ok(())
    }
}

impl RequestSchema {
    /// Creates a new [`RequestSchema`] without properties
    #[inline]
    pub fn new() -> Self {
        Self::default()   
    }
    
    /// Creates a new [`RequestSchema`] from a type that implements [`Default`] and [`Serialize`]
    #[inline]   
    pub fn of<T: JsonSchema>() -> Self {
        let mut schema = Self::default();
        let json_schema = schemars::schema_for!(T);
        let required = json_schema
            .get("required")
            .and_then(|v| v.as_array());
        if let Some(props) = json_schema
            .get("properties")
            .and_then(|v| v.as_object()) {
            for (field, def) in props {
                let req = required
                    .map(|arr| !arr.iter().any(|v| v == field))
                    .unwrap_or(true);
                schema = if req {
                    schema.with_required(field, Schema::from(def))
                } else {
                    schema.with_prop(field, Schema::from(def))
                }
            }
        }
        schema
    }

    /// Creates a new [`RequestSchema`] with a single optional property
    #[inline]
    pub fn with_prop(mut self, prop: &str, schema: impl Into<Schema>) -> Self {
        self.properties.insert(prop.into(), schema.into());
        self
    }
    
    /// Creates a new [`RequestSchema`] with a single required property
    #[inline]
    pub fn with_required(mut self, prop: &str, schema: impl Into<Schema>) -> Self {
        self = self.with_prop(prop, schema);
        self.required
            .get_or_insert_with(Vec::new)
            .push(prop.into());
        self
    }
}

impl ElicitResult {
    /// Creates a new accepted [`ElicitResult`]
    #[inline]
    pub fn accept() -> Self {
        Self {
            action: ElicitationAction::Accept,
            content: None,
        }
    }

    /// Creates a new declined [`ElicitResult`]
    #[inline]
    pub fn decline() -> Self {
        Self {
            action: ElicitationAction::Decline,
            content: None,
        }
    }
    
    /// Creates a new canceled [`ElicitResult`]
    #[inline]
    pub fn cancel() -> Self {
        Self {
            action: ElicitationAction::Cancel,
            content: None,
        }
    }
    
    /// Sets the content of the [`ElicitResult`]
    #[inline]   
    pub fn with_content<T: Serialize>(mut self, content: T) -> Self {
        self.content = Some(serde_json::to_value(&content).unwrap());
        self
    }
    
    /// Deserializes the content of the [`ElicitResult`]
    #[inline]  
    pub fn content<T: DeserializeOwned>(&self) -> Option<T> {
        self.content
            .as_ref()
            .and_then(|content| serde_json::from_value(content.clone()).ok())
    }
    
    /// Returns _true_ if the [`ElicitResult`] is accepted
    pub fn is_accepted(&self) -> bool {
        self.action == ElicitationAction::Accept
    }
    
    /// Returns _true_ if the [`ElicitResult`] is canceled
    pub fn is_canceled(&self) -> bool {
        self.action == ElicitationAction::Cancel
    }
    
    /// Returns _true_ if the [`ElicitResult`] is declined
    pub fn is_declined(&self) -> bool {
        self.action == ElicitationAction::Decline
    }
}

impl IntoResponse for ElicitResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        Response::success(req_id, serde_json::to_value(self).unwrap())
    }
}

/// Represents a dynamic handler for handling sampling requests
#[cfg(feature = "client")]
pub(crate) type ElicitationHandler = Arc<
    dyn Fn(ElicitRequestParams) -> Pin<
        Box<dyn Future<Output = ElicitResult> + Send + 'static>
    >
    + Send
    + Sync
>;

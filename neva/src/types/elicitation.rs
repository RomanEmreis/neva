//! Utilities for Elicitation

#[cfg(feature = "client")]
use std::{future::Future, pin::Pin, sync::Arc};
use std::collections::HashMap;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use schemars::JsonSchema;
use crate::{
    types::{IntoResponse, PropertyType, RequestId, Response, Schema},
    error::{Error, ErrorCode},
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

/// Represents a validator for elicitation content
pub struct Validator {
    schema: RequestSchema,
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

impl Validator {
    /// Creates a new [`Validator`]
    #[inline] 
    pub fn new(params: ElicitRequestParams) -> Self {
        Self {
            schema: params.schema,
        }
    }
    
    /// Validates the elicitation content against the schema
    #[inline]
    pub fn validate<T: Serialize + JsonSchema>(&self, content: T) -> Result<Value, Error> {
        let source_schema = schemars::schema_for!(T);
        self.validate_schema_compatibility(&source_schema)?;
        serde_json::to_value(&content)
            .map_err(Error::from)
            .and_then(|c| self.validate_content_constraints(&c).map(|_| c))
    }

    /// Validates that the source schema is compatible with the target schema
    fn validate_schema_compatibility(&self, source: &schemars::Schema) -> Result<(), Error> {
        const PROP: &str = "properties";
        const REQ: &str = "required";
        
        let target = &self.schema;
        let source_props = source
            .get(PROP)
            .and_then(|v| v.as_object())
            .ok_or(Error::new(ErrorCode::InvalidParams, "Source schema missing properties"))?;

        let source_required = source
            .get(REQ)
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
            .unwrap_or_default();

        // Check if all target properties exist in a source
        for prop_name in target.properties.keys() {
            if !source_props.contains_key(prop_name) {
                return Err(Error::new(
                    ErrorCode::InvalidParams, 
                    format!("Missing property: {prop_name}")));
            }
        }

        // Check if all required properties in the target are present
        if let Some(target_required) = &target.required {
            for required_prop in target_required {
                if !source_required.contains(&required_prop.as_str()) {
                    return Err(Error::new(
                        ErrorCode::InvalidParams, 
                        format!("Required property not marked as required: {required_prop}")));
                }
            }
        }

        Ok(())
    }

    /// Validates content against schema constraints
    fn validate_content_constraints(&self, content: &Value) -> Result<(), Error> {
        let schema = &self.schema;
        let content_obj = content
            .as_object()
            .ok_or(Error::new(ErrorCode::InvalidParams, "Content is not an object"))?;

        // Check required properties
        if let Some(required) = &schema.required {
            for required_prop in required {
                if !content_obj.contains_key(required_prop) {
                    return Err(Error::new(
                        ErrorCode::InvalidParams, 
                        format!("Missing required property: {required_prop}")));
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
    fn validate_property_value(&self, value: &Value, schema: &Schema) -> Result<(), Error> {
        match schema {
            Schema::String(string_schema) => {
                let str_value = value.as_str()
                    .ok_or(Error::new(ErrorCode::InvalidParams, "Expected string value"))?;

                if let Some(min_len) = string_schema.min_length {
                    if str_value.len() < min_len {
                        return Err(Error::new(
                            ErrorCode::InvalidParams, 
                            format!("String too short: {} < {min_len}", str_value.len())));
                    }
                }

                if let Some(max_len) = string_schema.max_length {
                    if str_value.len() > max_len {
                        return Err(Error::new(
                            ErrorCode::InvalidParams, 
                            format!("String too long: {} > {max_len}", str_value.len())));
                    }
                }

                // Validate format if specified
                if let Some(format) = &string_schema.format {
                    self.validate_string_format(str_value, format)?;
                }
            },
            Schema::Number(number_schema) => {
                let num_value = value.as_f64()
                    .ok_or(Error::new(ErrorCode::InvalidParams, "Expected number value"))?;

                if let Some(min) = number_schema.min {
                    if num_value < min {
                        return Err(Error::new(
                            ErrorCode::InvalidParams, 
                            format!("Number too small: {num_value} < {min}")));
                    }
                }

                if let Some(max) = number_schema.max {
                    if num_value > max {
                        return Err(
                            Error::new(
                                ErrorCode::InvalidParams, 
                                format!("Number too large: {num_value} > {max}")));
                    }
                }
            },
            Schema::Boolean(_) => {
                if !value.is_boolean() {
                    return Err(Error::new(ErrorCode::InvalidParams, "Expected boolean value"));
                }
            },
            Schema::Enum(enum_schema) => {
                let str_value = value.as_str()
                    .ok_or(Error::new(ErrorCode::InvalidParams, "Expected string value for enum"))?;

                if !enum_schema.r#enum.contains(&str_value.to_string()) {
                    return Err(Error::new(
                        ErrorCode::InvalidParams, 
                        format!("Invalid enum value: {str_value}")));
                }
            },
        }

        Ok(())
    }

    /// Validates a string format (basic validation for common formats)
    fn validate_string_format(&self, value: &str, format: &str) -> Result<(), Error> {
        match format {
            "email" => {
                if !value.contains('@') || !value.contains('.') {
                    return Err(Error::new(ErrorCode::InvalidParams, "Invalid email format"));
                }
            },
            "uri" => {
                if !value.starts_with("http://") && !value.starts_with("https://") && !value.starts_with("file://") {
                    return Err(Error::new(ErrorCode::InvalidParams, "Invalid URI format"));
                }
            },
            "date" => {
                // Basic date format validation (YYYY-MM-DD)
                if value.len() != 10 || value.chars().nth(4) != Some('-') || value.chars().nth(7) != Some('-') {
                    return Err(Error::new(ErrorCode::InvalidParams, "Invalid date format (expected YYYY-MM-DD)"));
                }
            },
            "date-time" => {
                if !value.contains('T') {
                    return Err(Error::new(ErrorCode::InvalidParams, "Invalid date format"));
                }
            },
            _ => {
                // Unknown format, skip validation
            }
        }
        Ok(())
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

    /// Maps the content of an accepted [`ElicitResult`] to a new value using the provided function.
    /// If the result is not accepted, returns an error.
    pub fn map<T, U, F>(&self, f: F) -> Result<U, Error>
    where
        T: DeserializeOwned,
        F: FnOnce(T) -> U,
    {
        if self.is_accepted() {
            self.content::<T>()
                .ok_or_else(|| Error::new(ErrorCode::ParseError, "Failed to parse content"))
                .map(f)
        } else {
            Err(Error::new(ErrorCode::InvalidRequest, "User rejected the request"))
        }
    }

    /// Maps the error of a declined or canceled [`ElicitResult`] using the provided function.
    /// If the result is accepted, returns Ok with the content.
    pub fn map_err<T, F>(&self, f: F) -> Result<T, Error>
    where
        T: DeserializeOwned,
        F: FnOnce() -> Error,
    {
        if self.is_accepted() {
            self.content::<T>()
                .ok_or_else(|| Error::new(ErrorCode::ParseError, "Failed to parse content"))
        } else {
            Err(f())
        }
    }

}

impl From<Result<Value, Error>> for ElicitResult {
    fn from(result: Result<Value, Error>) -> Self {
        match result {
            Ok(content) => ElicitResult::accept().with_content(content),
            Err(err) => ElicitResult::decline().with_content(err.to_string()),
        }
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

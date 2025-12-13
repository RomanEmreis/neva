//! Utilities for Elicitation

#[cfg(feature = "client")]
use std::{future::Future, pin::Pin, sync::Arc};
use std::collections::HashMap;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use schemars::JsonSchema;
use crate::{
    types::{IntoResponse, PropertyType, RequestId, Response, Schema, ErrorDetails},
    types::notification::Notification,
    error::{Error, ErrorCode},
};

use crate::types::Uri;

#[cfg(feature = "tasks")]
use crate::types::TaskMetadata;

/// List of commands for Elicitation
pub mod commands {
    /// Command name for creating a new elicitation request
    pub const CREATE: &str = "elicitation/create";
    
    /// Notification name for indicates the completion of elicitation
    pub const COMPLETE: &str = "notifications/elicitation/complete";
}

/// Represents a message issued from the server to elicit additional information from the user via the client.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ElicitRequestParams {
    /// Elicitation request parameters for a form
    Form(ElicitRequestFormParams),
    
    /// Elicitation request parameters for a URL
    Url(ElicitRequestUrlParams)
}

/// Represents the parameters for a request to elicit non-sensitive information from the user 
/// via a form in the client.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitRequestFormParams {
    /// The message to present to the user.
    pub message: String,

    /// The elicitation mode
    pub mode: Option<ElicitationMode>,

    /// The requested schema.
    ///
    /// > **Note:** A restricted subset of JSON Schema.
    /// > Only top-level properties are allowed, without nesting.
    #[serde(rename = "requestedSchema")]
    pub schema: RequestSchema,

    /// If specified, the caller is requesting task-augmented execution for this request.
    /// The request will return a [`CreateTaskResult`] immediately, and the actual result can be
    /// retrieved later via `tasks/result`.
    ///
    /// **Note:** Task augmentation is subject to capability negotiation - receivers **MUST** declare support
    /// for task augmentation of specific request types in their capabilities.
    #[cfg(feature = "tasks")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskMetadata>,

    /// Additional metadata to attach to the request.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>
}

/// Represents the parameters for a request to elicit information from the user 
/// via a URL in the client.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitRequestUrlParams {
    /// The ID of the elicitation, which must be unique within the context of the server.
    /// 
    /// The client **MUST** treat this ID as an opaque value.
    #[serde(rename = "elicitationId")]
    pub id: String,
    
    /// The message to present to the user.
    pub message: String,

    /// The elicitation mode
    pub mode: ElicitationMode,
    
    /// The URL that the user should navigate to.
    pub url: Uri,

    /// If specified, the caller is requesting task-augmented execution for this request.
    /// The request will return a [`CreateTaskResult`] immediately, and the actual result can be
    /// retrieved later via `tasks/result`.
    ///
    /// **Note:** Task augmentation is subject to capability negotiation - receivers **MUST** declare support
    /// for task augmentation of specific request types in their capabilities.
    #[cfg(feature = "tasks")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskMetadata>,
    
    /// Additional metadata to attach to the request.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>
}

/// Represents elicitation mode.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ElicitationMode {
    /// `form` elicitation mode
    Form,

    /// `url` elicitation mode
    Url
}

/// Represents a JSON Schema that can be used to validate the content of an elicitation request.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Represents an error response that indicates that the server requires the client
/// to provide additional information via an elicitation request.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlElicitationRequiredError {
    /// A list of required elicitations
    pub elicitations: Vec<ElicitRequestUrlParams>
}

/// Represents an optional notification from the server to the client, informing it of a completion 
/// of an out-of-band elicitation request.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationCompleteParams {
    /// The ID of the elicitation that completed.
    #[serde(rename = "elicitationId")]
    pub id: String,
}

/// Represents a validator for elicitation content
#[derive(Debug)]
pub struct Validator {
    schema: RequestSchema,
}

impl From<ElicitRequestFormParams> for ElicitRequestParams {
    #[inline]
    fn from(value: ElicitRequestFormParams) -> Self {
        Self::Form(value)
    }
}

impl From<ElicitRequestUrlParams> for ElicitRequestParams {
    #[inline]
    fn from(value: ElicitRequestUrlParams) -> Self {
        Self::Url(value)
    }
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
    pub fn new(params: ElicitRequestFormParams) -> Self {
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
    #[inline]
    fn validate_property_value(&self, value: &Value, schema: &Schema) -> Result<(), Error> {
        match schema {
            Schema::String(string_schema) => string_schema.validate(value),
            Schema::Number(number_schema) => number_schema.validate(value),
            Schema::Boolean(boolean_schema) => boolean_schema.validate(value),
            Schema::SingleUntitledEnum(e) => e.validate(value),
            Schema::SingleTitledEnum(e) => e.validate(value),
            Schema::MultiUntitledEnum(e) => e.validate(value),
            Schema::MultiTitledEnum(e) => e.validate(value),
            Schema::LegacyEnum(e) => e.validate(value),
        }
    }
}

impl ElicitRequestParams {
    /// Creates a new form [`ElicitRequestParams`]
    #[inline]
    pub fn form(message: impl Into<String>) -> ElicitRequestFormParams {
        ElicitRequestFormParams {
            message: message.into(),
            schema: RequestSchema::new(),
            mode: None,
            meta: None,
            #[cfg(feature = "tasks")]
            task: None
        }
    }

    /// Creates a new URL [`ElicitRequestParams`]
    #[inline]
    pub fn url(url: impl Into<Uri>, message: impl Into<String>) -> ElicitRequestUrlParams {
        ElicitRequestUrlParams {
            id: uuid::Uuid::new_v4().to_string(),
            message: message.into(),
            url: url.into(),
            mode: ElicitationMode::Url,
            meta: None,
            #[cfg(feature = "tasks")]
            task: None
        }
    }

    /// Returns a reference to the underlying [`ElicitRequestFormParams`] if the request is a form, 
    /// otherwise returns `None`
    #[inline]
    pub fn as_form(&self) -> Option<&ElicitRequestFormParams> {
        match self { 
            Self::Form(params) => Some(params),
            _ => None
        }
    }
    
    /// Returns a reference to the underlying [`ElicitRequestUrlParams`] if the request is a URL, 
    /// otherwise returns `None`
    #[inline]
    pub fn as_url(&self) -> Option<&ElicitRequestUrlParams> {
        match self { 
            Self::Url(params) => Some(params),
            _ => None
        }   
    }
    
    /// Converts the request into a form request. 
    /// Returns an error if the request is not a form request.
    #[inline]
    pub fn into_form(self) -> Result<ElicitRequestFormParams, Error> {
        match self {
            Self::Form(params) => Ok(params),
            _ => Err(Error::new(ErrorCode::InvalidRequest, "Request is not a form request"))
        }
    }

    /// Converts the request into a URL request. 
    /// Returns an error if the request is not a URL request.
    #[inline]
    pub fn into_url(self) -> Result<ElicitRequestUrlParams, Error> {
        match self {
            Self::Url(params) => Ok(params),
            _ => Err(Error::new(ErrorCode::InvalidRequest, "Request is not a URL request"))
        }
    }
}

impl ElicitRequestFormParams {
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

#[cfg(feature = "tasks")]
impl ElicitRequestUrlParams {
    /// Makes the request task-augmented with TTL.
    /// 
    /// Default: `None`
    pub fn with_task(mut self, ttl: Option<usize>) -> Self {
        self.task = Some(TaskMetadata { ttl });
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

impl UrlElicitationRequiredError {
    /// Creates a new [`UrlElicitationRequiredError`]
    #[inline]
    pub fn new(elicitations: impl IntoIterator<Item = ElicitRequestUrlParams>) -> Self {
        Self { elicitations: elicitations.into_iter().collect() }
    }
    
    /// Converts into JSONRPC error response
    #[inline] 
    pub fn to_error(self, message: impl Into<String>) -> Error {
        let err = match serde_json::to_value(self) {
            Ok(data) => ErrorDetails {
                code: ErrorCode::UrlElicitationRequiredError,
                message: message.into(),
                data: Some(data),
            },
            Err(err) => ErrorDetails {
                code: ErrorCode::InternalError,
                message: err.to_string(),
                data: None,
            }
        };
        err.into()
    }
}

impl ElicitationCompleteParams {
    /// Creates a new [`ElicitationCompleteParams`]
    #[inline]
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }
}

impl TryFrom<Notification> for ElicitationCompleteParams {
    type Error = Error;

    #[inline]
    fn try_from(value: Notification) -> Result<Self, Self::Error> {
        let params = value.params
            .ok_or_else(|| Error::new(ErrorCode::InvalidParams, "Missing params"))?;
        serde_json::from_value(params).map_err(Error::from)
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
        match serde_json::to_value(self) {
            Ok(v) => Response::success(req_id, v),
            Err(err) => Response::error(req_id, err.into())
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        StringSchema, StringFormat, 
        NumberSchema, 
        BooleanSchema, 
        UntitledSingleSelectEnumSchema
    };
    use schemars::JsonSchema;

    #[derive(Serialize, JsonSchema)]
    struct TestStruct {
        name: String,
        age: u32,
        active: bool,
    }

    fn create_test_schema() -> RequestSchema {
        let mut schema = RequestSchema::new();
        schema.properties.insert(
            "name".to_string(),
            Schema::String(StringSchema {
                r#type: PropertyType::String,
                title: None,
                descr: None,
                min_length: Some(2),
                max_length: Some(50),
                format: None,
            })
        );
        schema.properties.insert(
            "age".to_string(),
            Schema::Number(NumberSchema {
                r#type: PropertyType::Number,
                title: None,
                descr: None,
                min: Some(0.0),
                max: Some(120.0),
            })
        );
        schema.properties.insert(
            "active".to_string(),
            Schema::Boolean(BooleanSchema::default())
        );
        schema.required = Some(vec!["name".to_string(), "age".to_string()]);
        schema
    }

    fn create_form_params_with_schema(schema: RequestSchema) -> ElicitRequestFormParams {
        ElicitRequestFormParams {
            message: "Test message".to_string(),
            mode: None,
            meta: None,
            #[cfg(feature = "tasks")]
            task: None,
            schema,
        }
    }

    #[test]
    fn it_creates_validator_for_params_with_schema() {
        let schema = create_test_schema();
        let params = create_form_params_with_schema(schema.clone());
        let validator = Validator::new(params);

        assert_eq!(validator.schema.properties.len(), schema.properties.len());
        assert_eq!(validator.schema.required, schema.required);
    }

    #[test]
    fn it_validates_compatible_schema_success() {
        let schema = create_test_schema();
        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content = TestStruct {
            name: "John Doe".to_string(),
            age: 30,
            active: true,
        };

        let result = validator.validate(content);
        assert!(result.is_ok());

        let json_value = result.unwrap();
        assert_eq!(json_value["name"], "John Doe");
        assert_eq!(json_value["age"], 30);
        assert_eq!(json_value["active"], true);
    }

    #[test]
    fn it_validates_missing_property_in_source() {
        let mut schema = create_test_schema();
        schema.properties.insert(
            "missing_prop".to_string(),
            Schema::String(StringSchema::default())
        );

        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content = TestStruct {
            name: "John Doe".to_string(),
            age: 30,
            active: true,
        };

        let result = validator.validate(content);
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidParams);
        assert!(error.to_string().contains("Missing property: missing_prop"));
    }

    #[test]
    fn it_validates_missing_required_property() {
        let mut schema = create_test_schema();
        schema.required = Some(vec!["name".to_string(), "age".to_string(), "missing_required".to_string()]);

        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content = TestStruct {
            name: "John Doe".to_string(),
            age: 30,
            active: true,
        };

        let result = validator.validate(content);
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidParams);
        assert!(error.to_string().contains("Required property not marked as required"));
    }

    #[test]
    fn it_validates_content_constraints_missing_required() {
        let schema = create_test_schema();
        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        // Create content missing the required field
        let content_json = serde_json::json!({
            "active": true
            // Missing required "name" and "age" fields
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidParams);
        assert!(error.to_string().contains("Missing required property"));
    }

    #[test]
    fn it_validates_content_constraints_not_object() {
        let schema = create_test_schema();
        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content_json = serde_json::json!("not an object");

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidParams);
        assert!(error.to_string().contains("Content is not an object"));
    }

    #[test]
    fn it_validates_string_property_success() {
        let schema = create_test_schema();
        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content_json = serde_json::json!({
            "name": "John",
            "age": 25,
            "active": true
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_ok());
    }

    #[test]
    fn it_validates_string_property_too_short() {
        let schema = create_test_schema();
        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content_json = serde_json::json!({
            "name": "J", // Too short (min_length is 2)
            "age": 25,
            "active": true
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert!(error.to_string().contains("String too short: 1 < 2"));
    }

    #[test]
    fn it_validates_string_property_too_long() {
        let schema = create_test_schema();
        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let long_name = "a".repeat(51); // Too long (max_length is 50)
        let content_json = serde_json::json!({
            "name": long_name,
            "age": 25,
            "active": true
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert!(error.to_string().contains("String too long: 51 > 50"));
    }

    #[test]
    fn it_validates_string_property_invalid_type() {
        let schema = create_test_schema();
        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content_json = serde_json::json!({
            "name": 123, // Should be string
            "age": 25,
            "active": true
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert!(error.to_string().contains("Expected string value"));
    }

    #[test]
    fn it_validates_number_property_success() {
        let schema = create_test_schema();
        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content_json = serde_json::json!({
            "name": "John",
            "age": 50, // Within range [0, 120]
            "active": true
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_ok());
    }

    #[test]
    fn it_validates_number_property_too_small() {
        let schema = create_test_schema();
        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content_json = serde_json::json!({
            "name": "John",
            "age": -5, // Below minimum (0)
            "active": true
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert!(error.to_string().contains("Number too small: -5 < 0"));
    }

    #[test]
    fn it_validates_number_property_too_large() {
        let schema = create_test_schema();
        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content_json = serde_json::json!({
            "name": "John",
            "age": 150, // Above maximum (120)
            "active": true
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert!(error.to_string().contains("Number too large: 150 > 120"));
    }

    #[test]
    fn it_validatess_number_property_invalid_type() {
        let schema = create_test_schema();
        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content_json = serde_json::json!({
            "name": "John",
            "age": "not a number", // Should be number
            "active": true
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert!(error.to_string().contains("Expected number value"));
    }

    #[test]
    fn it_validates_boolean_property_success() {
        let schema = create_test_schema();
        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content_json = serde_json::json!({
            "name": "John",
            "age": 25,
            "active": false
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_ok());
    }

    #[test]
    fn it_validates_boolean_property_invalid_type() {
        let schema = create_test_schema();
        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content_json = serde_json::json!({
            "name": "John",
            "age": 25,
            "active": "not a boolean" // Should be boolean
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert!(error.to_string().contains("Expected boolean value"));
    }

    #[test]
    fn it_validates_enum_property_success() {
        let mut schema = RequestSchema::new();
        schema.properties.insert(
            "status".to_string(),
            Schema::SingleUntitledEnum(UntitledSingleSelectEnumSchema {
                r#type: PropertyType::String,
                title: None,
                descr: None,
                r#enum: vec!["active".to_string(), "inactive".to_string(), "pending".to_string()],
                default: None,
            })
        );
        schema.required = Some(vec!["status".to_string()]);

        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content_json = serde_json::json!({
            "status": "active"
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_ok());
    }

    #[test]
    fn it_validates_enum_property_invalid_value() {
        let mut schema = RequestSchema::new();
        schema.properties.insert(
            "status".to_string(),
            Schema::SingleUntitledEnum(UntitledSingleSelectEnumSchema {
                r#type: PropertyType::String,
                title: None,
                descr: None,
                r#enum: vec!["active".to_string(), "inactive".to_string()],
                default: None,
            })
        );
        schema.required = Some(vec!["status".to_string()]);

        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content_json = serde_json::json!({
            "status": "invalid_status"
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert!(error.to_string().contains("Invalid enum value: invalid_status"));
    }

    #[test]
    fn it_validates_enum_property_invalid_type() {
        let mut schema = RequestSchema::new();
        schema.properties.insert(
            "status".to_string(),
            Schema::SingleUntitledEnum(UntitledSingleSelectEnumSchema {
                r#type: PropertyType::String,
                title: None,
                descr: None,
                r#enum: vec!["active".to_string(), "inactive".to_string()],
                default: None,
            })
        );
        schema.required = Some(vec!["status".to_string()]);

        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content_json = serde_json::json!({
            "status": 123 // Should be string for enum
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert!(error.to_string().contains("Expected string value for enum"));
    }

    #[test]
    fn it_validates_string_format_email_success() {
        let mut schema = RequestSchema::new();
        schema.properties.insert(
            "email".to_string(),
            Schema::String(StringSchema {
                r#type: PropertyType::String,
                title: None,
                descr: None,
                min_length: None,
                max_length: None,
                format: Some(StringFormat::Email),
            })
        );
        schema.required = Some(vec!["email".to_string()]);

        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content_json = serde_json::json!({
            "email": "test@example.com"
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_ok());
    }

    #[test]
    fn it_validates_string_format_email_invalid() {
        let mut schema = RequestSchema::new();
        schema.properties.insert(
            "email".to_string(),
            Schema::String(StringSchema {
                r#type: PropertyType::String,
                title: None,
                descr: None,
                min_length: None,
                max_length: None,
                format: Some(StringFormat::Email),
            })
        );
        schema.required = Some(vec!["email".to_string()]);

        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content_json = serde_json::json!({
            "email": "invalid-email"
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert!(error.to_string().contains("Invalid email format"));
    }

    #[test]
    fn it_validates_string_format_uri_success() {
        let mut schema = RequestSchema::new();
        schema.properties.insert(
            "website".to_string(),
            Schema::String(StringSchema {
                r#type: PropertyType::String,
                title: None,
                descr: None,
                min_length: None,
                max_length: None,
                format: Some(StringFormat::Uri),
            })
        );

        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let test_cases = vec![
            "http://example.com",
            "https://example.com",
            "file://path/to/file",
            "res://resource_1"
        ];

        for uri in test_cases {
            let content_json = serde_json::json!({
                "website": uri
            });

            let result = validator.validate_content_constraints(&content_json);
            assert!(result.is_ok(), "Failed for URI: {}", uri);
        }
    }

    #[test]
    fn it_validates_string_format_uri_invalid() {
        let mut schema = RequestSchema::new();
        schema.properties.insert(
            "website".to_string(),
            Schema::String(StringSchema {
                r#type: PropertyType::String,
                title: None,
                descr: None,
                min_length: None,
                max_length: None,
                format: Some(StringFormat::Uri),
            })
        );

        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content_json = serde_json::json!({
            "website": "not-a-uri"
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert!(error.to_string().contains("Invalid URI format"));
    }

    #[test]
    fn it_validates_string_format_date_success() {
        let mut schema = RequestSchema::new();
        schema.properties.insert(
            "birth_date".to_string(),
            Schema::String(StringSchema {
                r#type: PropertyType::String,
                title: None,
                descr: None,
                min_length: None,
                max_length: None,
                format: Some(StringFormat::Date),
            })
        );

        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content_json = serde_json::json!({
            "birth_date": "1990-05-15"
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_ok());
    }

    #[test]
    fn it_validates_string_format_date_invalid() {
        let mut schema = RequestSchema::new();
        schema.properties.insert(
            "birth_date".to_string(),
            Schema::String(StringSchema {
                r#type: PropertyType::String,
                title: None,
                descr: None,
                min_length: None,
                max_length: None,
                format: Some(StringFormat::Date),
            })
        );

        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let test_cases = vec![
            "1990/05/15",     // Wrong separators
            "90-05-15",       // Wrong year format
            "1990-5-15",      // Missing zero padding
            "not-a-date",     // Invalid format
        ];

        for invalid_date in test_cases {
            let content_json = serde_json::json!({
                "birth_date": invalid_date
            });

            let result = validator.validate_content_constraints(&content_json);
            assert!(result.is_err(), "Should fail for invalid date: {}", invalid_date);

            let error = result.unwrap_err();
            assert!(error.to_string().contains("Invalid date format"));
        }
    }

    #[test]
    fn it_validates_string_format_datetime_success() {
        let mut schema = RequestSchema::new();
        schema.properties.insert(
            "updated_at".to_string(),
            Schema::String(StringSchema {
                r#type: PropertyType::String,
                title: None,
                descr: None,
                min_length: None,
                max_length: None,
                format: Some(StringFormat::DateTime),
            })
        );

        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content_json = serde_json::json!({
            "updated_at": "2023-05-15T14:30:00Z"
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_ok());
    }

    #[test]
    fn it_validates_string_format_datetime_invalid() {
        let mut schema = RequestSchema::new();
        schema.properties.insert(
            "updated_at".to_string(),
            Schema::String(StringSchema {
                r#type: PropertyType::String,
                title: None,
                descr: None,
                min_length: None,
                max_length: None,
                format: Some(StringFormat::DateTime),
            })
        );

        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content_json = serde_json::json!({
            "updated_at": "2023-05-15 14:30:00" // Missing 'T' separator
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert!(error.to_string().contains("Invalid date format"));
    }

    #[test]
    fn it_validates_string_format_unknown_format() {
        let mut schema = RequestSchema::new();
        schema.properties.insert(
            "custom_field".to_string(),
            Schema::String(StringSchema {
                r#type: PropertyType::String,
                title: None,
                descr: None,
                min_length: None,
                max_length: None,
                format: None,
            })
        );

        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content_json = serde_json::json!({
            "custom_field": "any value should work"
        });

        // Unknown formats should be skipped and pass validation
        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_ok());
    }

    #[test]
    fn it_validates_optional_properties() {
        let mut schema = RequestSchema::new();
        schema.properties.insert(
            "required_field".to_string(),
            Schema::String(StringSchema::default())
        );
        schema.properties.insert(
            "optional_field".to_string(),
            Schema::String(StringSchema::default())
        );
        schema.required = Some(vec!["required_field".to_string()]);

        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        // Test with only the required field
        let content_json = serde_json::json!({
            "required_field": "value"
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_ok());

        // Test with both required and optional fields
        let content_json = serde_json::json!({
            "required_field": "value",
            "optional_field": "optional_value"
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_ok());
    }

    #[test]
    fn it_validates_no_required_properties() {
        let mut schema = RequestSchema::new();
        schema.properties.insert(
            "optional_field".to_string(),
            Schema::String(StringSchema::default())
        );
        // No required fields
        schema.required = None;

        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content_json = serde_json::json!({});

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_ok());
    }

    #[test]
    fn it_validates_schema_compatibility_no_properties() {
        let schema = RequestSchema::new(); // Empty schema
        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content = TestStruct {
            name: "John Doe".to_string(),
            age: 30,
            active: true,
        };

        // Should succeed since the target schema has no requirements
        let result = validator.validate(content);
        assert!(result.is_ok());
    }

    #[test]
    fn it_tests_serialize_error_handling() {
        let schema = create_test_schema();
        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        // This would normally cause a serialization error if we had a type that can't serialize
        // For this test, we'll use a valid serializable type
        let content = TestStruct {
            name: "John Doe".to_string(),
            age: 30,
            active: true,
        };

        let result = validator.validate(content);
        assert!(result.is_ok());
    }

    #[test]
    fn it_tests_request_schema_default() {
        let schema = RequestSchema::default();

        assert_eq!(schema.r#type, PropertyType::Object);
        assert!(schema.properties.is_empty());
        assert_eq!(schema.required, None);
    }

    #[test]
    fn it_tests_edge_case_empty_enum() {
        let mut schema = RequestSchema::new();
        schema.properties.insert(
            "status".to_string(),
            Schema::SingleUntitledEnum(UntitledSingleSelectEnumSchema {
                r#type: PropertyType::String,
                title: None,
                descr: None,
                r#enum: vec![],
                default: None,
            })
        );

        let params = create_form_params_with_schema(schema);
        let validator = Validator::new(params);

        let content_json = serde_json::json!({
            "status": "any_value"
        });

        let result = validator.validate_content_constraints(&content_json);
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert!(error.to_string().contains("Invalid enum value"));
    }
}
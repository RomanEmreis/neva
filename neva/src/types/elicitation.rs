//! Utilities for Elicitation

#[cfg(feature = "client")]
use std::{future::Future, pin::Pin, sync::Arc};
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::types::{IntoResponse, PropertyType, RequestId, Response, Schema};

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
    pub content: Option<HashMap<String, Value>>,
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
}

impl RequestSchema {
    /// Creates a new [`RequestSchema`] without properties
    #[inline]
    pub fn new() -> Self {
        Self::default()   
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

//! Types and utils for references

use std::fmt::Display;
use serde::{Serialize, Deserialize};

/// Represents a reference to a resource or prompt.
/// Umbrella type for both ResourceReference and PromptReference from the spec schema.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details 
#[derive(Serialize, Deserialize)]
pub struct Reference {
    /// The type of content. Can be ref/resource or ref/prompt.
    #[serde(rename = "type")]
    pub r#type: String,
    
    /// The URI or URI template of the resource.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    
    /// The name of the prompt or prompt template.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Display for Reference {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.uri, &self.name) {
            (Some(uri), None) => write!(f, "{}: {}", self.r#type, uri),
            (None, Some(name)) => write!(f, "{}: {}", self.r#type, name),
            _ => write!(f, "{}: unknown", self.r#type),
        }
    }
}

impl Reference {
    /// Creates a ref/resource [`Reference`]
    #[inline]
    pub fn resource(uri: &str) -> Self {
        Self {
            r#type: "ref/resource".into(),
            uri: Some(uri.into()),
            name: None
        }
    }

    /// Creates a ref/resource [`Reference`]
    #[inline]
    pub fn prompt(name: &str) -> Self {
        Self {
            r#type: "ref/prompt".into(),
            name: Some(name.into()),
            uri: None
        }
    }
    
    /// Validates the reference object.
    /// 
    /// # Example
    /// ```no_run
    /// use neva::types::Reference;
    /// 
    /// // valid ref/resource
    /// let reference = Reference::resource("file://test");
    /// assert!(reference.validate().is_none());
    ///
    /// // valid ref/prompt
    /// let reference = Reference::prompt("test");
    /// assert!(reference.validate().is_none())
    /// ```
    pub fn validate(&self) -> Option<String> {
        match self.r#type.as_ref() {
            "ref/resource" => if self.uri.is_none() { Some("uri is required for ref/resource".into()) } else { None },
            "ref/prompt" => if self.name.is_none() { Some("name is required for ref/prompt".into()) } else { None },
            _ => Some(format!("unknown reference type: {}", self.r#type)),
        }
    }
}
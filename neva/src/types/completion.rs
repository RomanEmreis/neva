//! Completion request types

use serde::{Deserialize, Serialize};
use super::Reference;
#[cfg(feature = "server")]
use crate::error::Error;

#[cfg(feature = "server")]
use crate::types::request::FromRequest;
#[cfg(feature = "server")]
use crate::app::handler::{FromHandlerParams, HandlerParams};
#[cfg(feature = "server")]
use super::{IntoResponse, RequestId, Response, Request};

/// List of commands for Completion
pub mod commands {
    /// Command name that returns autocompletion options.
    pub const COMPLETE: &str = "completion/complete";
}

/// Represents a completion object in the server's response
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Debug, Serialize, Deserialize)]
pub struct Completion {
    /// An array of completion values. Must not exceed 100 items.
    pub values: Vec<String>,
    
    /// The total number of completion options available. 
    /// This can exceed the number of values actually sent in the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<usize>,
    
    /// Indicates whether there are additional completion options beyond those provided
    /// in the current response, even if the exact total is unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
}

/// A request from the client to the server to ask for completion options.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Serialize, Deserialize)]
pub struct CompleteRequestParams {
    /// The reference's information
    #[serde(rename = "ref")]
    pub r#ref: Reference,
    
    /// The argument's information
    #[serde(rename = "argument")]
    pub arg: Argument,
}

/// Used for completion requests to provide additional context for the completion options.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Serialize, Deserialize)]
pub struct Argument {
    /// The name of the argument.
    pub name: String,
    
    /// The value of the argument to use for completion matching.
    pub value: String,
}

/// The server's response to a completion/complete request
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CompleteResult {
    /// The completion object containing the completion values.
    pub completion: Completion,
}

impl Default for Completion {
    #[inline]
    fn default() -> Self {
        Self {
            values: vec![],
            total: Some(0),
            has_more: Some(false),
        }
    }
}

#[cfg(feature = "server")]
impl FromHandlerParams for CompleteRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

#[cfg(feature = "server")]
impl Completion {
    /// Creates a new empty [`Completion`] object
    #[inline]
    pub fn new<T, V>(values: T, total: usize) -> Self
    where 
        T: IntoIterator<Item = V>,
        V: Into<String>,
    {
        let values: Vec<String> = values
            .into_iter()
            .map(Into::into)
            .collect();
        Self {
            total: Some(total),
            has_more: Some(total > values.len()),
            values,
        }
    }
}

#[cfg(feature = "server")]
impl CompleteResult {
    /// Create a new [`CompleteResult`] object
    #[inline]
    pub fn new(completion: Completion) -> Self {
        Self { completion }
    }
}

#[cfg(feature = "server")]
impl IntoResponse for CompleteResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        match serde_json::to_value(self) {
            Ok(v) => Response::success(req_id, v),
            Err(err) => Response::error(req_id, err.into())
        }
    }
}

#[cfg(feature = "server")]
impl From<String> for Completion {
    #[inline]
    fn from(val: String) -> Self {
        Self { 
            values: vec![val], 
            total: None,
            has_more: None 
        }
    }
}

#[cfg(feature = "server")]
impl From<&str> for Completion {
    #[inline]
    fn from(val: &str) -> Self {
        Self {
            values: vec![val.into()],
            total: None,
            has_more: None
        }
    }
}

#[cfg(feature = "server")]
impl<T, E> TryFrom<Result<T, E>> for CompleteResult
where
    T: Into<CompleteResult>,
    E: Into<Error>
{
    type Error = E;

    #[inline]
    fn try_from(value: Result<T, E>) -> Result<Self, Self::Error> {
        match value {
            Ok(ok) => Ok(ok.into()),
            Err(err) => Err(err)
        }
    }
}

#[cfg(feature = "server")]
impl<T> From<T> for CompleteResult
where
    T: Into<Completion>
{
    #[inline]
    fn from(val: T) -> Self {
        CompleteResult::new(val.into())
    }
}

#[cfg(feature = "server")]
impl<T> From<Option<T>> for CompleteResult 
where
    T: Into<Completion>
{
    #[inline]
    fn from(value: Option<T>) -> Self {
        match value { 
            Some(val) => CompleteResult::new(val.into()),
            None => CompleteResult::default()
        }
    }
}

#[cfg(feature = "server")]
impl From<Vec<String>> for Completion {
    #[inline]
    fn from(vec: Vec<String>) -> Self {
        let len = vec.len();
        Self {
            values: vec,
            total: Some(len),
            has_more: Some(false),
        }
    }
}

#[cfg(feature = "server")]
impl From<Vec<&str>> for Completion {
    #[inline]
    fn from(vec: Vec<&str>) -> Self {
        let len = vec.len();
        Self {
            total: Some(len),
            has_more: Some(false),
            values: vec
                .into_iter()
                .map(String::from)
                .collect(),
        }
    }
}

#[cfg(feature = "server")]
impl<const N: usize> From<[String; N]> for Completion {
    #[inline]
    fn from(arr: [String; N]) -> Self {
        let len = arr.len();
        Self {
            values: arr.to_vec(),
            total: Some(len),
            has_more: Some(false),
        }
    }
}

#[cfg(feature = "server")]
impl<const N: usize> From<[&str; N]> for Completion {
    #[inline]
    fn from(arr: [&str; N]) -> Self {
        let len = arr.len();
        Self {
            total: Some(len),
            has_more: Some(false),
            values: arr
                .into_iter()
                .map(String::from)
                .collect(),
        }
    }
}

#[cfg(test)]
#[cfg(feature = "server")]
mod tests {
    use super::*;
    
    #[test]
    fn it_creates_default_completion() {
        let completion = Completion::default();
        
        assert_eq!(completion.values.len(), 0);
        assert_eq!(completion.total, Some(0));
        assert_eq!(completion.has_more, Some(false));
    }

    #[test]
    fn it_creates_new_completion() {
        let completion = Completion::new(["1", "2", "3"], 5);

        assert_eq!(completion.values.len(), 3);
        assert_eq!(completion.total, Some(5));
        assert_eq!(completion.has_more, Some(true));
    }
    
    #[test]
    fn it_converts_complete_result_into_response() {
        let result = CompleteResult::default();
        
        let resp = result.into_response(RequestId::default());
        let json = serde_json::to_string(&resp).unwrap();

        assert_eq!(json, r#"{"jsonrpc":"2.0","id":"(no id)","result":{"completion":{"has_more":false,"total":0,"values":[]}}}"#);
    }
    
    #[test]
    fn it_converts_vec_into_completion() {
        let vec = vec!["1", "2", "3"];
        let completion: Completion = vec.into();
        
        assert_eq!(completion.values.len(), 3);
        assert_eq!(completion.total, Some(3));
        assert_eq!(completion.has_more, Some(false));
    }

    #[test]
    fn it_converts_vec_into_completion_result() {
        let vec = vec!["1", "2", "3"];
        let completion: CompleteResult = vec.into();

        assert_eq!(completion.completion.values.len(), 3);
        assert_eq!(completion.completion.total, Some(3));
        assert_eq!(completion.completion.has_more, Some(false));
    }

    #[test]
    fn it_converts_array_into_completion() {
        let vec = ["1", "2", "3"];
        let completion: Completion = vec.into();

        assert_eq!(completion.values.len(), 3);
        assert_eq!(completion.total, Some(3));
        assert_eq!(completion.has_more, Some(false));
    }

    #[test]
    fn it_converts_array_into_completion_result() {
        let vec = ["1", "2", "3"];
        let completion: CompleteResult = vec.into();

        assert_eq!(completion.completion.values.len(), 3);
        assert_eq!(completion.completion.total, Some(3));
        assert_eq!(completion.completion.has_more, Some(false));
    }
}
//! Types and utils for prompt request results

use serde::{Serialize, Deserialize};
use crate::types::{Content, Role};
#[cfg(feature = "server")]
use crate::{error::Error, types::{IntoResponse, RequestId, Response}};
    
/// The server's response to a prompts/get request from the client.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Default, Serialize, Deserialize)]
pub struct GetPromptResult {
    /// An optional description for the prompt.
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,

    /// The prompt or prompt template that the server offers.
    pub messages: Vec<PromptMessage>,
}

/// Describes a message returned as part of a prompt.
///
/// This is similar to `SamplingMessage`, but also supports the embedding of 
/// resources from the MCP server.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Debug, Serialize, Deserialize)]
pub struct PromptMessage {
    /// The content of the message. Any of TextContent, ImageContent, EmbeddedResource.
    pub content: Content,

    /// The role of the message ("user" or "assistant").
    pub role: Role,
}

#[cfg(feature = "server")]
impl IntoResponse for GetPromptResult {
    fn into_response(self, req_id: RequestId) -> Response {
        Response::success(req_id, serde_json::to_value(self).unwrap())
    }
}

#[cfg(feature = "server")]
impl<T: Into<Role>> From<(&str, T)> for PromptMessage {
    #[inline]
    fn from((msg, role): (&str, T)) -> Self {
        Self::text(msg, role.into())
    }
}

#[cfg(feature = "server")]
impl<T: Into<Role>> From<(String, T)> for PromptMessage {
    #[inline]
    fn from((msg, role): (String, T)) -> Self {
        Self {
            content: msg.into(),
            role: role.into(),
        }
    }
}

#[cfg(feature = "server")]
impl<T> From<T> for GetPromptResult
where 
    T: Into<PromptMessage>
{
    #[inline]
    fn from(msg: T) -> Self {
        Self { descr: None, messages: vec![msg.into()] }
    }
}

#[cfg(feature = "server")]
impl<T, E> TryFrom<Result<T, E>> for GetPromptResult
where 
    T: Into<GetPromptResult>,
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
impl<T> From<Vec<T>> for GetPromptResult
where
    T: Into<PromptMessage>
{
    #[inline]
    fn from(iter: Vec<T>) -> Self {
        Self {
            descr: None,
            messages: iter
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

#[cfg(feature = "server")]
impl<const N: usize, T> From<[T; N]> for GetPromptResult
where
    T: Into<PromptMessage>
{
    #[inline]
    fn from(iter: [T; N]) -> Self {
        Self {
            descr: None,
            messages: iter
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

#[cfg(feature = "server")]
impl PromptMessage {
    /// Creates a new [`PromptMessage`]
    #[inline]
    pub fn new(content: Content, role: Role) -> Self {
        Self { content, role }
    }
    
    #[inline]
    pub fn text(content: &str, role: Role) -> Self {
        Self {
            content: content.into(),
            role,
        }
    }
}

#[cfg(feature = "server")]
impl GetPromptResult {
    /// Creates a new [`GetPromptResult`]
    #[inline]
    pub fn new(descr: Option<String>, messages: impl Iterator<Item = PromptMessage>) -> Self {
        Self { descr, messages: messages.collect() }
    }
}

#[cfg(test)]
mod tests {
    
}
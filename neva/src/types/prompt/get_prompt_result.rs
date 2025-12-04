//! Types and utils for prompt request results

use serde::{Serialize, Deserialize};
use crate::types::{Content, Role};
#[cfg(feature = "server")]
use crate::{error::Error, types::{IntoResponse, RequestId, Response}};
    
/// The server's response to a prompts/get request from the client.
///
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2024-11-05/schema.json) for details
#[derive(Debug, Default, Serialize, Deserialize)]
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
        match serde_json::to_value(self) {
            Ok(v) => Response::success(req_id, v),
            Err(err) => Response::error(req_id, err.into())
        }
    }
}

#[cfg(feature = "server")]
impl<T1, T2> From<(T1, T2)> for PromptMessage
where 
    T1: Into<Content>,
    T2: Into<Role>
{
    #[inline]
    fn from((msg, role): (T1, T2)) -> Self {
        Self::new(role).with(msg)
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
    pub fn new(role: impl Into<Role>) -> Self {
        Self { 
            content: Content::empty(), 
            role: role.into()
        }
    }
    
    /// Creates a new [`PromptMessage`] with the user role
    pub fn user() -> Self {
        Self::new(Role::User)
    }
    
    /// Creates a new [`PromptMessage`] with the assistant role
    pub fn assistant() -> Self {
        Self::new(Role::Assistant)
    }
    
    /// Sets the content of [`PromptMessage`]
    pub fn with<T: Into<Content>>(mut self, content: T) -> Self {
        self.content = content.into();
        self
    }
}

#[cfg(feature = "server")]
impl GetPromptResult {
    /// Creates a new [`GetPromptResult`]
    #[inline]
    pub fn new() -> Self {
        Self { 
            messages: Vec::with_capacity(8),
            descr: None
        }
    }
    
    /// Sets the description of the result
    pub fn with_descr<T: Into<String>>(mut self, descr: T) -> Self {
        self.descr = Some(descr.into());
        self
    }

    /// Adds a message to the result
    pub fn with_message<T: Into<PromptMessage>>(mut self, message: T) -> Self {
        self.messages.push(message.into());
        self
    }
    
    /// Adds multiple messages to the result
    pub fn with_messages<T, I>(mut self, messages: T) -> Self
    where 
        T: IntoIterator<Item = I>,
        I: Into<PromptMessage>
    {
        self.messages
            .extend(messages.into_iter().map(Into::into));
        self
    }
}

#[cfg(test)]
mod tests {
    
}
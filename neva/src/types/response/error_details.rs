//! Represents error details utils for JSON-RPC responses

use crate::error::{Error, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::json;

const INTERNAL_CODE_KEY: &str = "__neva_internal_code";

/// Detailed error information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorDetails {
    /// Integer error code.
    pub code: ErrorCode,

    /// Short description of the error.
    pub message: String,

    /// Optional additional error data.
    pub data: Option<serde_json::Value>,
}

impl Default for ErrorDetails {
    #[inline]
    fn default() -> Self {
        Self {
            code: ErrorCode::InternalError,
            message: "Unknown error".into(),
            data: None,
        }
    }
}

impl From<Error> for ErrorDetails {
    #[inline]
    fn from(err: Error) -> Self {
        Self {
            code: err.code.wire_code(),
            message: err.to_string(),
            data: None,
        }
    }
}

impl From<ErrorDetails> for Error {
    #[inline]
    fn from(details: ErrorDetails) -> Self {
        let code = details.internal_code().unwrap_or(details.code);
        Error::new(code, details.message)
    }
}

impl ErrorDetails {
    /// Creates a new [`ErrorDetails`]
    #[inline]
    pub fn new(err: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::InternalError,
            message: err.into(),
            data: None,
        }
    }

    #[inline]
    pub(crate) fn timeout() -> Self {
        Self {
            code: ErrorCode::InternalError,
            message: ErrorCode::Timeout.to_string(),
            data: Some(json!({
                INTERNAL_CODE_KEY: i32::from(ErrorCode::Timeout),
            })),
        }
    }

    #[inline]
    pub(crate) fn internal_code(&self) -> Option<ErrorCode> {
        let value = self.data.as_ref()?.get(INTERNAL_CODE_KEY)?.as_i64()?;
        i32::try_from(value)
            .ok()
            .and_then(|value| value.try_into().ok())
    }
}

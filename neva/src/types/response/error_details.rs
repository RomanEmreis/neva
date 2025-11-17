//! Represents error details utils for JSON-RPC responses

use serde::{Deserialize, Serialize};
use crate::error::{Error, ErrorCode};

/// Detailed error information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorDetails {
    /// Integer error code.
    pub code: ErrorCode,

    /// Short description of the error.
    pub message: String,

    /// Optional additional error data.
    pub data: Option<serde_json::Value>
}

impl Default for ErrorDetails {
    #[inline]
    fn default() -> Self {
        Self {
            code: ErrorCode::InternalError,
            message: "Unknown error".into(),
            data: None
        }
    }
}

impl From<Error> for ErrorDetails {
    #[inline]
    fn from(err: Error) -> Self {
        Self { 
            code: err.code, 
            message: err.to_string(), 
            data: None
        }
    }
}

impl From<ErrorDetails> for Error {
    #[inline]
    fn from(details: ErrorDetails) -> Self {
        Error::new(details.code, details.message)
    }
}

impl ErrorDetails {
    /// Creates a new [`ErrorDetails`]
    #[inline]
    pub fn new(err: &str) -> Self {
        Self { 
            code: ErrorCode::InternalError, 
            message: err.into(), 
            data: None
        }
    }
}
//! Represents error details utils for JSON-RPC responses

use crate::error::{Error, ErrorCode};
use serde::{Deserialize, Serialize};

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
            data: err.data.clone(),
        }
    }
}

impl From<ErrorDetails> for Error {
    #[inline]
    fn from(details: ErrorDetails) -> Self {
        // The payload is half of what the MCP-allocated errors say -- the
        // versions on offer, the capabilities a server needs -- so dropping it
        // here would leave every client command holding a bare message.
        let err = Error::new(details.code, details.message);
        match details.data {
            Some(data) => err.with_data(data),
            None => err,
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `data` payload has to survive both directions, or a client sees the
    /// message of an MCP-allocated error without the part it can act on.
    #[test]
    fn the_data_payload_round_trips() {
        let data = serde_json::json!({ "supported": ["2026-07-28"], "requested": "1999-01-01" });
        let err = Error::new(ErrorCode::InternalError, "nope").with_data(data.clone());

        let details = ErrorDetails::from(err);
        assert_eq!(details.data.as_ref(), Some(&data));

        let back = Error::from(details);
        assert_eq!(back.data(), Some(&data));
    }

    #[test]
    fn an_error_without_data_stays_without_data() {
        let details = ErrorDetails::from(Error::new(ErrorCode::InvalidParams, "nope"));
        assert!(details.data.is_none());
        assert!(Error::from(details).data().is_none());
    }
}

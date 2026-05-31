//! Represents an error

use std::convert::Infallible;
use std::error::Error as StdError;
use std::fmt;
use std::io::Error as IoError;

pub use error_code::ErrorCode;

pub mod error_code;

type BoxError = Box<dyn StdError + Send + Sync>;

/// Represents MCP server error
#[derive(Debug)]
pub struct Error {
    pub(crate) code: ErrorCode,
    inner: BoxError,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(self.inner.as_ref())
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Error {
        Self {
            inner: err.into(),
            code: ErrorCode::ParseError,
        }
    }
}

impl From<IoError> for Error {
    fn from(err: IoError) -> Error {
        Self {
            inner: err.into(),
            code: ErrorCode::InternalError,
        }
    }
}

impl From<Infallible> for Error {
    fn from(infallible: Infallible) -> Error {
        match infallible {}
    }
}

impl Error {
    /// Creates a new [`Error`]
    #[inline]
    pub fn new(code: impl TryInto<ErrorCode>, err: impl Into<BoxError>) -> Self {
        Self {
            inner: err.into(),
            code: code.try_into().unwrap_or_default(),
        }
    }

    /// Builds the internal MRTR "input required" sentinel error.
    ///
    /// Returned by `Context::elicit` on a cache miss to unwind the handler;
    /// the actual pending request is carried in the shared MRTR context.
    #[cfg(feature = "proto-2026-07-28-rc")]
    pub(crate) fn input_required() -> Self {
        Self::new(ErrorCode::InputRequired, "input required")
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "proto-2026-07-28-rc")]
    use super::*;

    #[cfg(feature = "proto-2026-07-28-rc")]
    #[test]
    fn input_required_sentinel_carries_the_sentinel_code() {
        assert_eq!(Error::input_required().code, ErrorCode::InputRequired);
    }
}

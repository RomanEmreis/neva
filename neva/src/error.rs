//! Represents an error

use std::convert::Infallible;
use std::fmt;
use std::error::Error as StdError;
use std::io::Error as IoError;

pub use error_code::ErrorCode;

pub mod error_code;

type BoxError = Box<
    dyn StdError
    + Send
    + Sync
>;

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
            code: ErrorCode::ParseError
         }
    }
}

impl From<IoError> for Error {
    fn from(err: IoError) -> Error {
        Self { 
            inner: err.into(),
            code: ErrorCode::InternalError
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
    pub fn new(code: impl TryInto<ErrorCode>, err: impl Into<BoxError>) -> Error {
        Self { 
            inner: err.into(),
            code: code
                .try_into()
                .unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests {
    
}


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
    /// Structured payload carried into the JSON-RPC error object's `data`.
    ///
    /// MCP 2026-07-28 specifies a `data` shape for some errors (the supported
    /// versions on an unsupported-version rejection, the capabilities a server
    /// needs on a missing-capability rejection), so the code alone is not the
    /// whole error.
    pub(crate) data: Option<serde_json::Value>,
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
            data: None,
        }
    }
}

impl From<IoError> for Error {
    fn from(err: IoError) -> Error {
        Self {
            inner: err.into(),
            code: ErrorCode::InternalError,
            data: None,
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
            data: None,
        }
    }

    /// Attaches a structured `data` payload to this error.
    ///
    /// # Example
    /// ```
    /// use neva::error::{Error, ErrorCode};
    ///
    /// let err = Error::new(ErrorCode::InvalidParams, "bad city")
    ///     .with_data(serde_json::json!({ "field": "city" }));
    /// # let _ = err;
    /// ```
    #[inline]
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }

    /// The structured `data` payload this error carries, if any.
    ///
    /// Set by [`Self::with_data`] on the way out, and preserved on the way in:
    /// an error decoded from a peer's response keeps what the peer sent, which
    /// is where the MCP-allocated errors put the part that is actionable --
    /// `supported` / `requested` on an unsupported version,
    /// `requiredCapabilities` on a missing capability.
    ///
    /// # Example
    /// ```
    /// use neva::error::{Error, ErrorCode};
    ///
    /// let err = Error::new(ErrorCode::InvalidParams, "bad city")
    ///     .with_data(serde_json::json!({ "field": "city" }));
    ///
    /// assert_eq!(err.data().and_then(|d| d.get("field")), Some(&"city".into()));
    /// ```
    #[inline]
    pub fn data(&self) -> Option<&serde_json::Value> {
        self.data.as_ref()
    }

    /// Builds the internal MRTR "input required" sentinel error.
    ///
    /// Returned by `Context::elicit` on a cache miss to unwind the handler;
    /// the actual pending request is carried in the shared MRTR context.
    /// Server-only: the client never constructs this sentinel.
    #[cfg(all(not(feature = "legacy-spec"), feature = "server"))]
    pub(crate) fn input_required() -> Self {
        Self::new(ErrorCode::InputRequired, "input required")
    }
}

#[cfg(test)]
mod tests {
    #[cfg(all(not(feature = "legacy-spec"), feature = "server"))]
    use super::*;

    #[cfg(all(not(feature = "legacy-spec"), feature = "server"))]
    #[test]
    fn input_required_sentinel_carries_the_sentinel_code() {
        assert_eq!(Error::input_required().code, ErrorCode::InputRequired);
    }
}

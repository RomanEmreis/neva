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

/// A model's failure, called through svir. The message is svir's own
/// account, which leaves out what the model server said: this error may be
/// what an MCP server is answered with, and a provider's message can name the
/// account. The svir error stays the source, `server_message` included.
#[cfg(feature = "svir")]
impl From<::svir::Error> for Error {
    fn from(err: ::svir::Error) -> Error {
        let code = match err.kind() {
            ::svir::ErrorKind::Timeout => ErrorCode::Timeout,
            ::svir::ErrorKind::Unsupported => ErrorCode::InvalidParams,
            _ => ErrorCode::InternalError,
        };
        Self {
            inner: err.into(),
            code,
            data: None,
        }
    }
}

impl Error {
    /// Creates a new [`Error`]
    ///
    /// The code is an [`ErrorCode`] or its number; a number neva has no
    /// variant for is kept as [`ErrorCode::Other`].
    ///
    /// # Example
    /// ```
    /// use neva::error::{Error, ErrorCode};
    ///
    /// assert_eq!(Error::new(ErrorCode::InvalidParams, "bad").code(), ErrorCode::InvalidParams);
    /// assert_eq!(Error::new(-32000, "not ready").code(), ErrorCode::Other(-32000));
    /// ```
    #[inline]
    pub fn new(code: impl Into<ErrorCode>, err: impl Into<BoxError>) -> Self {
        Self {
            inner: err.into(),
            code: code.into(),
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

    /// The code of this error: what went wrong, for a caller to branch on.
    ///
    /// An error decoded from a peer's response keeps the code the peer sent,
    /// and [`Self::data`] holds what goes with it, such as `missingScopes` on
    /// [`ErrorCode::InsufficientScope`]. An error neva raises on its own side
    /// may carry one of its internal codes, [`ErrorCode::Timeout`] or
    /// [`ErrorCode::RequestCancelled`], which never travel on the wire.
    ///
    /// A code neva has no variant for, such as a server's own, arrives as
    /// [`ErrorCode::Other`] with the number the peer sent, its message and
    /// `data` intact.
    ///
    /// # Example
    /// ```
    /// use neva::error::{Error, ErrorCode};
    ///
    /// let err = Error::new(ErrorCode::InsufficientScope, "insufficient scope")
    ///     .with_data(serde_json::json!({ "missingScopes": ["memory:write"] }));
    ///
    /// match err.code() {
    ///     ErrorCode::InsufficientScope => {
    ///         let missing = &err.data().unwrap()["missingScopes"];
    ///         assert_eq!(missing[0], "memory:write");
    ///     }
    ///     other => panic!("unexpected {other}"),
    /// }
    /// ```
    #[inline]
    pub fn code(&self) -> ErrorCode {
        self.code
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
    #[cfg(any(
        all(not(feature = "legacy-spec"), feature = "server"),
        feature = "svir"
    ))]
    use super::*;

    #[cfg(all(not(feature = "legacy-spec"), feature = "server"))]
    #[test]
    fn input_required_sentinel_carries_the_sentinel_code() {
        assert_eq!(Error::input_required().code, ErrorCode::InputRequired);
    }

    /// What an MCP server is told of a model's failure is svir's own account
    /// of it: the model server's message stays with the caller.
    #[cfg(feature = "svir")]
    #[test]
    fn a_model_failure_leaves_the_model_servers_message_out() {
        let failure = ::svir::Error::new(::svir::ErrorKind::Authentication)
            .with_server_message("Incorrect API key provided: sk-abc...xyz");

        let err = Error::from(failure);
        assert_eq!(err.code, ErrorCode::InternalError);
        assert_eq!(err.to_string(), "the model server refused the credentials");

        let source = StdError::source(&err)
            .and_then(|source| source.downcast_ref::<::svir::Error>())
            .expect("the svir error is the source");
        assert!(
            source
                .server_message()
                .is_some_and(|m| m.contains("sk-abc"))
        );
    }

    #[cfg(feature = "svir")]
    #[test]
    fn a_model_failure_keeps_what_kind_it_was() {
        for (kind, code) in [
            (::svir::ErrorKind::Timeout, ErrorCode::Timeout),
            (::svir::ErrorKind::Unsupported, ErrorCode::InvalidParams),
            (::svir::ErrorKind::ContextOverflow, ErrorCode::InternalError),
        ] {
            assert_eq!(Error::from(::svir::Error::new(kind)).code, code);
        }
    }
}

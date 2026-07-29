//! Represents error code tools

use crate::error::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::Display;

/// Standard JSON-RPC error codes as defined in the MCP specification.
#[derive(Default, Debug, Copy, Clone, Eq, PartialEq)]
pub enum ErrorCode {
    /// The server received invalid JSON.
    ParseError = -32700,

    /// The JSON sent is not a valid Request object.
    InvalidRequest = -32600,

    /// The method does not exist / is not available.
    MethodNotFound = -32601,

    /// Invalid method parameter(s).
    InvalidParams = -32602,

    /// Internal JSON-RPC error.
    #[default]
    InternalError = -32603,

    /// The resource does not exist / is not available.
    #[deprecated(note = "use InvalidParams")]
    ResourceNotFound = -32002,

    /// The URL mode elicitation is required.
    UrlElicitationRequiredError = -32042,

    /// The request's HTTP headers do not match the corresponding values in the
    /// body, or a required header is missing or malformed (MCP 2026-07-28).
    ///
    /// Over HTTP the response status must be `400 Bad Request`.
    #[cfg(not(feature = "legacy-spec"))]
    HeaderMismatch = -32020,

    /// Processing the request needs a capability the client did not declare in
    /// its per-request `clientCapabilities` (MCP 2026-07-28).
    ///
    /// The error `data` carries `requiredCapabilities`. Over HTTP the response
    /// status must be `400 Bad Request`.
    #[cfg(not(feature = "legacy-spec"))]
    MissingRequiredClientCapability = -32021,

    /// The request's protocol version is unknown to or unsupported by the
    /// server (MCP 2026-07-28).
    ///
    /// The error `data` carries `supported` and `requested`. Over HTTP the
    /// response status must be `400 Bad Request`.
    #[cfg(not(feature = "legacy-spec"))]
    UnsupportedProtocolVersion = -32022,

    /// [Internal code] The request has been canceled
    RequestCancelled = -99999,

    /// [Internal code] The request has been timed out
    Timeout = -99998,

    /// [Internal code] A handler requested additional input via MRTR. Never
    /// sent on the wire as an error -- intercepted by the server dispatch layer
    /// and converted into an `InputRequiredResult`.
    #[cfg(not(feature = "legacy-spec"))]
    InputRequired = -99997,
}

impl From<ErrorCode> for i32 {
    fn from(code: ErrorCode) -> Self {
        code as i32
    }
}

impl TryFrom<i32> for ErrorCode {
    type Error = ();

    #[inline]
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            -32700 => Ok(ErrorCode::ParseError),
            -32600 => Ok(ErrorCode::InvalidRequest),
            -32601 => Ok(ErrorCode::MethodNotFound),
            -32602 => Ok(ErrorCode::InvalidParams),
            -32603 => Ok(ErrorCode::InternalError),
            #[allow(deprecated)]
            -32002 => Ok(ErrorCode::ResourceNotFound),
            -32042 => Ok(ErrorCode::UrlElicitationRequiredError),
            #[cfg(not(feature = "legacy-spec"))]
            -32020 => Ok(ErrorCode::HeaderMismatch),
            #[cfg(not(feature = "legacy-spec"))]
            -32021 => Ok(ErrorCode::MissingRequiredClientCapability),
            #[cfg(not(feature = "legacy-spec"))]
            -32022 => Ok(ErrorCode::UnsupportedProtocolVersion),
            -99999 => Ok(ErrorCode::RequestCancelled),
            -99998 => Ok(ErrorCode::Timeout),
            #[cfg(not(feature = "legacy-spec"))]
            -99997 => Ok(ErrorCode::InputRequired),
            _ => Err(()),
        }
    }
}

// Implement serde::Serialize
impl Serialize for ErrorCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let code: i32 = (*self).into();
        serializer.serialize_i32(code)
    }
}

// Implement serde::Deserialize
impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<ErrorCode, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = i32::deserialize(deserializer)?;
        ErrorCode::try_from(value)
            .map_err(|_| serde::de::Error::custom(format!("Invalid error code: {value}")))
    }
}

impl Display for ErrorCode {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorCode::ParseError => write!(f, "Parse error"),
            ErrorCode::InvalidRequest => write!(f, "Invalid request"),
            ErrorCode::MethodNotFound => write!(f, "Method not found"),
            ErrorCode::InvalidParams => write!(f, "Invalid parameters"),
            ErrorCode::InternalError => write!(f, "Internal error"),
            #[allow(deprecated)]
            ErrorCode::ResourceNotFound => write!(f, "Resource not found"),
            ErrorCode::UrlElicitationRequiredError => write!(f, "URL elicitation required error"),
            #[cfg(not(feature = "legacy-spec"))]
            ErrorCode::HeaderMismatch => write!(f, "Header mismatch"),
            #[cfg(not(feature = "legacy-spec"))]
            ErrorCode::MissingRequiredClientCapability => {
                write!(f, "Missing required client capability")
            }
            #[cfg(not(feature = "legacy-spec"))]
            ErrorCode::UnsupportedProtocolVersion => write!(f, "Unsupported protocol version"),
            ErrorCode::RequestCancelled => write!(f, "Request cancelled"),
            ErrorCode::Timeout => write!(f, "Request timed out"),
            #[cfg(not(feature = "legacy-spec"))]
            ErrorCode::InputRequired => write!(f, "Input required"),
        }
    }
}

impl From<ErrorCode> for Error {
    fn from(code: ErrorCode) -> Self {
        Error::new(code, code.to_string())
    }
}

impl ErrorCode {
    /// Returns the wire-safe equivalent of this code.
    ///
    /// Internal codes (`RequestCancelled`, `Timeout`) fall outside the JSON-RPC 2.0
    /// reserved range (`-32768` to `-32000`) and must never appear in a response
    /// payload. This method maps them to [`ErrorCode::InternalError`] so callers can
    /// always serialise a spec-compliant code.
    ///
    /// Under MCP 2026-07-28 the deprecated [`Self::ResourceNotFound`]
    /// (`-32002`) is additionally remapped to [`Self::InvalidParams`] (`-32602`)
    /// per the 2026-07-28, so a user handler returning the old variant still serialises
    /// the spec-current code.
    ///
    /// All other standard codes are returned unchanged.
    ///
    /// # Example
    /// ```
    /// use neva::error::ErrorCode;
    ///
    /// assert_eq!(ErrorCode::RequestCancelled.wire_code(), ErrorCode::InternalError);
    /// assert_eq!(ErrorCode::Timeout.wire_code(), ErrorCode::InternalError);
    /// assert_eq!(ErrorCode::ParseError.wire_code(), ErrorCode::ParseError);
    /// ```
    #[inline]
    pub fn wire_code(self) -> Self {
        match self {
            Self::RequestCancelled | Self::Timeout => Self::InternalError,
            #[cfg(not(feature = "legacy-spec"))]
            Self::InputRequired => Self::InternalError,
            #[cfg(not(feature = "legacy-spec"))]
            #[allow(deprecated)]
            Self::ResourceNotFound => Self::InvalidParams,
            other => other,
        }
    }

    /// Code to use for "resource not found" -- spec-version dependent.
    ///
    /// - Default build (pre-2026 spec): [`Self::ResourceNotFound`] (`-32002`).
    /// - MCP 2026-07-28: [`Self::InvalidParams`] (`-32602`), per the 2026-07-28.
    ///
    /// This is the migration path for the now-deprecated
    /// [`Self::ResourceNotFound`] variant: reference this constant instead of
    /// naming the variant (or hard-coding [`Self::InvalidParams`]) so the wire
    /// code follows the active spec version automatically. All in-tree emitters
    /// use it; downstream handlers should too.
    ///
    /// # Example
    /// ```
    /// use neva::error::{Error, ErrorCode};
    ///
    /// // Prefer this over the deprecated `ErrorCode::ResourceNotFound`.
    /// let err = Error::new(ErrorCode::RESOURCE_NOT_FOUND, "no such resource");
    /// ```
    pub const RESOURCE_NOT_FOUND: Self = {
        #[cfg(not(feature = "legacy-spec"))]
        {
            Self::InvalidParams
        }
        #[cfg(feature = "legacy-spec")]
        {
            #[allow(deprecated)]
            Self::ResourceNotFound
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(deprecated)]
    fn it_converts_to_i32() {
        let codes = [
            (-32700, ErrorCode::ParseError),
            (-32600, ErrorCode::InvalidRequest),
            (-32601, ErrorCode::MethodNotFound),
            (-32602, ErrorCode::InvalidParams),
            (-32603, ErrorCode::InternalError),
            (-32002, ErrorCode::ResourceNotFound),
            (-32042, ErrorCode::UrlElicitationRequiredError),
            (-99999, ErrorCode::RequestCancelled),
            (-99998, ErrorCode::Timeout),
        ];

        for (code, val) in codes {
            let error: ErrorCode = code.try_into().unwrap();
            assert_eq!(error, val);

            let int: i32 = val.into();
            assert_eq!(int, code);
        }
    }

    #[test]
    #[allow(deprecated)]
    fn it_serializes_error_codes() {
        let codes = [
            ("-32700", ErrorCode::ParseError),
            ("-32600", ErrorCode::InvalidRequest),
            ("-32601", ErrorCode::MethodNotFound),
            ("-32602", ErrorCode::InvalidParams),
            ("-32603", ErrorCode::InternalError),
            ("-32002", ErrorCode::ResourceNotFound),
            ("-32042", ErrorCode::UrlElicitationRequiredError),
            ("-99999", ErrorCode::RequestCancelled),
            ("-99998", ErrorCode::Timeout),
        ];

        for (code, val) in codes {
            let error = serde_json::to_string(&val).unwrap();
            assert_eq!(error, code);

            let error_code: ErrorCode = serde_json::from_str(&error).unwrap();
            assert_eq!(error_code, val);
        }
    }

    #[test]
    fn internal_codes_map_to_internal_error_on_wire() {
        assert_eq!(
            ErrorCode::RequestCancelled.wire_code(),
            ErrorCode::InternalError
        );
        assert_eq!(ErrorCode::Timeout.wire_code(), ErrorCode::InternalError);
    }

    #[test]
    fn standard_codes_are_unchanged_on_wire() {
        let standard = [
            ErrorCode::ParseError,
            ErrorCode::InvalidRequest,
            ErrorCode::MethodNotFound,
            ErrorCode::InvalidParams,
            ErrorCode::InternalError,
        ];
        for code in standard {
            assert_eq!(code.wire_code(), code);
        }
    }

    #[test]
    #[allow(deprecated)]
    fn resource_not_found_wire_code_matches_spec_version() {
        #[cfg(not(feature = "legacy-spec"))]
        assert_eq!(
            ErrorCode::ResourceNotFound.wire_code(),
            ErrorCode::InvalidParams
        );

        #[cfg(feature = "legacy-spec")]
        assert_eq!(
            ErrorCode::ResourceNotFound.wire_code(),
            ErrorCode::ResourceNotFound
        );
    }

    #[test]
    fn resource_not_found_alias_matches_spec_version() {
        #[cfg(not(feature = "legacy-spec"))]
        assert_eq!(ErrorCode::RESOURCE_NOT_FOUND, ErrorCode::InvalidParams);

        #[cfg(feature = "legacy-spec")]
        {
            #[allow(deprecated)]
            let expected = ErrorCode::ResourceNotFound;
            assert_eq!(ErrorCode::RESOURCE_NOT_FOUND, expected);
        }
    }
}

/// The MCP-allocated error codes (`-32020`..) introduced in 2026-07-28.
#[cfg(test)]
#[cfg(not(feature = "legacy-spec"))]
mod spec_error_code_tests {
    use super::ErrorCode;

    #[test]
    fn codes_match_the_spec_allocation() {
        // The spec allocates `-32020`.. sequentially to MCP-defined errors;
        // `-32001`/`-32003`/`-32004` were the pre-final numbers and must not
        // resurface.
        assert_eq!(i32::from(ErrorCode::HeaderMismatch), -32020);
        assert_eq!(
            i32::from(ErrorCode::MissingRequiredClientCapability),
            -32021
        );
        assert_eq!(i32::from(ErrorCode::UnsupportedProtocolVersion), -32022);
    }

    #[test]
    fn codes_round_trip_through_the_wire() {
        for code in [
            ErrorCode::HeaderMismatch,
            ErrorCode::MissingRequiredClientCapability,
            ErrorCode::UnsupportedProtocolVersion,
        ] {
            // They sit inside the JSON-RPC reserved range, so they travel as
            // themselves rather than being masked as an internal error.
            assert_eq!(code.wire_code(), code);
            assert_eq!(ErrorCode::try_from(i32::from(code)), Ok(code));
        }
    }
}

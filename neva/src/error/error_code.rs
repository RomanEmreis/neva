//! Represents error code tools

use crate::error::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::Display;

/// JSON-RPC error codes: the ones the JSON-RPC and MCP specifications define,
/// neva's own, and [`ErrorCode::Other`] for any other number.
///
/// New codes are added as MCP allocates them, so a `match` needs a `_` arm.
/// A number neva names converts to its variant, and every other number to
/// [`ErrorCode::Other`]:
///
/// ```
/// use neva::error::ErrorCode;
///
/// assert_eq!(ErrorCode::from(-32602), ErrorCode::InvalidParams);
/// assert_eq!(ErrorCode::from(-32000), ErrorCode::Other(-32000));
/// assert_eq!(i32::from(ErrorCode::Other(-32000)), -32000);
/// ```
#[non_exhaustive]
#[derive(Default, Debug, Copy, Clone, Eq, PartialEq)]
pub enum ErrorCode {
    /// `-32700`: the server received invalid JSON.
    ParseError,

    /// `-32600`: the JSON sent is not a valid Request object.
    InvalidRequest,

    /// `-32601`: the method does not exist / is not available.
    MethodNotFound,

    /// `-32602`: invalid method parameter(s).
    InvalidParams,

    /// `-32603`: internal JSON-RPC error.
    #[default]
    InternalError,

    /// `-32002`: the resource does not exist / is not available.
    #[deprecated(note = "use InvalidParams")]
    ResourceNotFound,

    /// `-32042`: the URL mode elicitation is required.
    UrlElicitationRequiredError,

    /// `-32020`: the request's HTTP headers do not match the corresponding
    /// values in the body, or a required header is missing or malformed (MCP
    /// 2026-07-28).
    ///
    /// Over HTTP the response status must be `400 Bad Request`.
    #[cfg(not(feature = "legacy-spec"))]
    HeaderMismatch,

    /// `-32021`: processing the request needs a capability the client did not
    /// declare in its per-request `clientCapabilities` (MCP 2026-07-28).
    ///
    /// The error `data` carries `requiredCapabilities`. Over HTTP the response
    /// status must be `400 Bad Request`.
    #[cfg(not(feature = "legacy-spec"))]
    MissingRequiredClientCapability,

    /// `-32022`: the request's protocol version is unknown to or unsupported
    /// by the server (MCP 2026-07-28).
    ///
    /// The error `data` carries `supported` and `requested`. Over HTTP the
    /// response status must be `400 Bad Request`.
    #[cfg(not(feature = "legacy-spec"))]
    UnsupportedProtocolVersion,

    /// `-31403`: the caller's access token is valid but does not grant every
    /// scope the tool, prompt or resource requires.
    ///
    /// MCP allocates no code for this, so neva takes one outside the range
    /// JSON-RPC reserves (`-32768` to `-32000`), where no later MCP allocation
    /// can land on it. The error `data` carries `requiredScopes`, every scope
    /// the item requires, and `missingScopes`, the ones the token lacks.
    ///
    /// Over HTTP the response status is `403 Forbidden`, with a
    /// `WWW-Authenticate: Bearer error="insufficient_scope"` challenge naming
    /// `requiredScopes` (RFC 6750 section 3.1): a client re-authorizes for them
    /// and retries.
    InsufficientScope,

    /// `-99999`, \[internal code\]: the request has been canceled.
    RequestCancelled,

    /// `-99998`, \[internal code\]: the request has been timed out.
    Timeout,

    /// `-99997`, \[internal code\]: a handler requested additional input via
    /// MRTR. Never sent on the wire as an error -- intercepted by the server
    /// dispatch layer and converted into an `InputRequiredResult`.
    #[cfg(not(feature = "legacy-spec"))]
    InputRequired,

    /// A code neva has no variant for, kept as the number the peer sent or the
    /// handler gave: an implementation-defined server error from the range
    /// JSON-RPC leaves to implementations (`-32000` to `-32099`), such as the
    /// TypeScript SDK's `-32000`, or a code of a server's own.
    ///
    /// [`ErrorCode::from`] returns this only for a number no other variant
    /// stands for; `Other(-32602)` built by hand is not equal to
    /// [`ErrorCode::InvalidParams`], though both travel as `-32602`.
    Other(i32),
}

impl From<ErrorCode> for i32 {
    fn from(code: ErrorCode) -> Self {
        match code {
            ErrorCode::ParseError => -32700,
            ErrorCode::InvalidRequest => -32600,
            ErrorCode::MethodNotFound => -32601,
            ErrorCode::InvalidParams => -32602,
            ErrorCode::InternalError => -32603,
            #[allow(deprecated)]
            ErrorCode::ResourceNotFound => -32002,
            ErrorCode::UrlElicitationRequiredError => -32042,
            #[cfg(not(feature = "legacy-spec"))]
            ErrorCode::HeaderMismatch => -32020,
            #[cfg(not(feature = "legacy-spec"))]
            ErrorCode::MissingRequiredClientCapability => -32021,
            #[cfg(not(feature = "legacy-spec"))]
            ErrorCode::UnsupportedProtocolVersion => -32022,
            ErrorCode::InsufficientScope => -31403,
            ErrorCode::RequestCancelled => -99999,
            ErrorCode::Timeout => -99998,
            #[cfg(not(feature = "legacy-spec"))]
            ErrorCode::InputRequired => -99997,
            ErrorCode::Other(code) => code,
        }
    }
}

impl From<i32> for ErrorCode {
    #[inline]
    fn from(value: i32) -> Self {
        match value {
            -32700 => ErrorCode::ParseError,
            -32600 => ErrorCode::InvalidRequest,
            -32601 => ErrorCode::MethodNotFound,
            -32602 => ErrorCode::InvalidParams,
            -32603 => ErrorCode::InternalError,
            #[allow(deprecated)]
            -32002 => ErrorCode::ResourceNotFound,
            -32042 => ErrorCode::UrlElicitationRequiredError,
            #[cfg(not(feature = "legacy-spec"))]
            -32020 => ErrorCode::HeaderMismatch,
            #[cfg(not(feature = "legacy-spec"))]
            -32021 => ErrorCode::MissingRequiredClientCapability,
            #[cfg(not(feature = "legacy-spec"))]
            -32022 => ErrorCode::UnsupportedProtocolVersion,
            -31403 => ErrorCode::InsufficientScope,
            -99999 => ErrorCode::RequestCancelled,
            -99998 => ErrorCode::Timeout,
            #[cfg(not(feature = "legacy-spec"))]
            -99997 => ErrorCode::InputRequired,
            other => ErrorCode::Other(other),
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
    /// Any number decodes: one neva does not name as [`ErrorCode::Other`].
    fn deserialize<D>(deserializer: D) -> Result<ErrorCode, D::Error>
    where
        D: Deserializer<'de>,
    {
        i32::deserialize(deserializer).map(ErrorCode::from)
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
            ErrorCode::InsufficientScope => write!(f, "Insufficient scope"),
            ErrorCode::RequestCancelled => write!(f, "Request cancelled"),
            ErrorCode::Timeout => write!(f, "Request timed out"),
            #[cfg(not(feature = "legacy-spec"))]
            ErrorCode::InputRequired => write!(f, "Input required"),
            ErrorCode::Other(code) => write!(f, "Error code {code}"),
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
    /// Internal codes (`RequestCancelled`, `Timeout`) are neva's own and must
    /// never appear in a response payload. This method maps them to [`ErrorCode::InternalError`] so callers can
    /// always serialise a spec-compliant code.
    ///
    /// Under MCP 2026-07-28 the deprecated [`Self::ResourceNotFound`]
    /// (`-32002`) is additionally remapped to [`Self::InvalidParams`] (`-32602`)
    /// per the 2026-07-28, so a user handler returning the old variant still serialises
    /// the spec-current code.
    ///
    /// All other standard codes, and [`Self::Other`] codes, are returned
    /// unchanged. An `Other` holding a number neva names is read as that
    /// variant first, so a hand-built `Other(-99999)` does not carry an
    /// internal code out either.
    ///
    /// # Example
    /// ```
    /// use neva::error::ErrorCode;
    ///
    /// assert_eq!(ErrorCode::RequestCancelled.wire_code(), ErrorCode::InternalError);
    /// assert_eq!(ErrorCode::Timeout.wire_code(), ErrorCode::InternalError);
    /// assert_eq!(ErrorCode::ParseError.wire_code(), ErrorCode::ParseError);
    /// assert_eq!(ErrorCode::Other(-32000).wire_code(), ErrorCode::Other(-32000));
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
            Self::Other(code) => match Self::from(code) {
                other @ Self::Other(_) => other,
                named => named.wire_code(),
            },
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
            (-31403, ErrorCode::InsufficientScope),
            (-99999, ErrorCode::RequestCancelled),
            (-99998, ErrorCode::Timeout),
        ];

        for (code, val) in codes {
            let error = ErrorCode::from(code);
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
            ("-31403", ErrorCode::InsufficientScope),
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

    /// A code neva has no variant for keeps its number, in both directions
    /// and through serde (#171).
    #[test]
    fn an_unknown_code_keeps_its_number() {
        for code in [-32000, -32099, -32768, -1, 0, 42, i32::MIN, i32::MAX] {
            assert_eq!(ErrorCode::from(code), ErrorCode::Other(code));
            assert_eq!(i32::from(ErrorCode::Other(code)), code);

            let decoded: ErrorCode = serde_json::from_str(&code.to_string()).unwrap();
            assert_eq!(decoded, ErrorCode::Other(code));
            assert_eq!(serde_json::to_string(&decoded).unwrap(), code.to_string());
        }
        assert_eq!(ErrorCode::Other(-32000).to_string(), "Error code -32000");
    }

    /// A number with a variant of its own converts to it, never to `Other`.
    #[test]
    #[allow(deprecated)]
    fn a_named_code_never_converts_to_other() {
        let named = [
            ErrorCode::ParseError,
            ErrorCode::InvalidRequest,
            ErrorCode::MethodNotFound,
            ErrorCode::InvalidParams,
            ErrorCode::InternalError,
            ErrorCode::ResourceNotFound,
            ErrorCode::UrlElicitationRequiredError,
            ErrorCode::InsufficientScope,
            ErrorCode::RequestCancelled,
            ErrorCode::Timeout,
        ];
        for code in named {
            assert_eq!(ErrorCode::from(i32::from(code)), code);
        }
    }

    /// Under `legacy-spec` the 2026-07-28 allocations have no variant, so a
    /// peer's `-32020` arrives as `Other` rather than failing to decode.
    #[cfg(feature = "legacy-spec")]
    #[test]
    fn a_later_allocation_arrives_as_other_under_legacy_spec() {
        assert_eq!(ErrorCode::from(-32020), ErrorCode::Other(-32020));
    }

    /// `Other` travels as written, unless it holds a number neva names: then
    /// it is that code, internal ones masked as usual.
    #[test]
    fn other_codes_are_unchanged_on_wire_unless_neva_names_them() {
        assert_eq!(
            ErrorCode::Other(-32000).wire_code(),
            ErrorCode::Other(-32000)
        );
        assert_eq!(
            ErrorCode::Other(-32602).wire_code(),
            ErrorCode::InvalidParams
        );
        assert_eq!(
            ErrorCode::Other(-99999).wire_code(),
            ErrorCode::InternalError
        );
    }

    #[test]
    fn internal_codes_map_to_internal_error_on_wire() {
        assert_eq!(
            ErrorCode::RequestCancelled.wire_code(),
            ErrorCode::InternalError
        );
        assert_eq!(ErrorCode::Timeout.wire_code(), ErrorCode::InternalError);
    }

    /// A later MCP allocation lands in the range JSON-RPC reserves, so a code
    /// of neva's own has to stay out of it -- and, unlike the internal codes,
    /// it is meant for the wire.
    #[test]
    fn insufficient_scope_travels_outside_the_reserved_range() {
        let code = i32::from(ErrorCode::InsufficientScope);
        assert!(!(-32768..=-32000).contains(&code));
        assert_eq!(
            ErrorCode::InsufficientScope.wire_code(),
            ErrorCode::InsufficientScope
        );
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
            assert_eq!(ErrorCode::from(i32::from(code)), code);
        }
    }
}

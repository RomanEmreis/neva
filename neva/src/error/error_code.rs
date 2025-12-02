//! Represents error code tools

use std::fmt::Display;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use crate::error::Error;

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
    ResourceNotFound = -32002,

    /// The URL mode elicitation is required.
    UrlElicitationRequiredError = -32042,
    
    /// [Internal code] The request has been canceled
    RequestCancelled = -99999,

    /// [Internal code] The request has been timed out
    Timeout = -99998,
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
            -32002 => Ok(ErrorCode::ResourceNotFound),
            -32042 => Ok(ErrorCode::UrlElicitationRequiredError),
            -99999 => Ok(ErrorCode::RequestCancelled),
            -99998 => Ok(ErrorCode::Timeout),
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
        ErrorCode::try_from(value).map_err(|_| {
            serde::de::Error::custom(format!("Invalid error code: {value}"))
        })
    }
}

impl Display for ErrorCode {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { 
            ErrorCode::ParseError => write!(f, "Parse error"),
            ErrorCode::InvalidRequest => write!(f, "Invalid request"),
            ErrorCode::MethodNotFound => write!(f, "Method not found"),
            ErrorCode::InvalidParams  => write!(f, "Invalid parameters"),
            ErrorCode::InternalError => write!(f, "Internal error"),
            ErrorCode::ResourceNotFound => write!(f, "Resource not found"),
            ErrorCode::UrlElicitationRequiredError => write!(f, "URL elicitation required error"),
            ErrorCode::RequestCancelled => write!(f, "Request cancelled"),
            ErrorCode::Timeout => write!(f, "Request timed out"),
        }
    }
}

impl From<ErrorCode> for Error {
    fn from(code: ErrorCode) -> Self {
        Error::new(code, code.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
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
}
//! Represents an error

use std::convert::Infallible;
use std::fmt;
use std::error::Error as StdError;
use std::io::Error as IoError;

type BoxError = Box<
    dyn StdError
    + Send
    + Sync
>;

/// Represents MCP server error
#[derive(Debug)]
pub struct Error {
    inner: BoxError
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
        Self { inner: err.into() }
    }
}

impl From<IoError> for Error {
    fn from(err: IoError) -> Error {
        Self { inner: err.into() }
    }
}

impl From<Infallible> for Error {
    fn from(infallible: Infallible) -> Error {
        match infallible {}
    }
}

impl Error {
    pub fn new(err: impl Into<BoxError>) -> Error {
        Self { inner: err.into() }
    }
}

#[cfg(test)]
mod tests {
    
}


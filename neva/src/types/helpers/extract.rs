//! Traits and helpers for type extraction from request arguments

use std::collections::hash_map::Iter;
use serde::de::DeserializeOwned;
use crate::Context;
use crate::error::{Error, ErrorCode};
use crate::types::{Meta, ProgressToken};
use crate::types::request::RequestParamsMeta;

/// Represents a payload that needs the type to be extracted from
pub(crate) enum Payload<'a> {
    /// Tool or Prompt argument
    Args(serde_json::Value),

    /// Request metadata ("_meta")
    Meta(&'a Option<RequestParamsMeta>),
}

/// Represents an extraction sources
pub(crate) enum Source {
    /// Tool or Prompt arguments
    Args,
    /// Request metadata ("_meta")
    Meta,
}

/// A trait that type needs to implement to be extractable from [`Request`]
pub(crate) trait RequestArgument: Sized {
    type Error;

    /// Extracts a type value from [`Payload`]
    fn extract(payload: Payload<'_>) -> Result<Self, Self::Error>;

    /// Returns a [`Source`] that the type needs to be extracted from
    #[inline]
    fn source() -> Source {
        Source::Args
    }
}

impl<'a> Payload<'a> {
    /// Returns arguments value for type extraction
    #[inline]
    pub(crate) fn expect_args(self) -> serde_json::Value {
        match self {
            Payload::Args(val) => val,
            _ => unreachable!("Expected Args variant"),
        }
    }

    /// Returns an optional [`RequestParamsMeta`] for type extraction
    #[inline]
    pub(crate) fn expect_meta(self) -> &'a Option<RequestParamsMeta> {
        match self {
            Payload::Meta(meta) => meta,
            _ => unreachable!("Expected Meta variant"),
        }
    }
}

impl<T: DeserializeOwned> RequestArgument for T {
    type Error = Error;

    #[inline]
    fn extract(payload: Payload<'_>) -> Result<Self, Self::Error> {
        let arg = payload.expect_args();
        T::deserialize(arg).map_err(Error::from)
    }
}

impl RequestArgument for Meta<RequestParamsMeta> {
    type Error = Error;

    #[inline]
    fn extract(payload: Payload<'_>) -> Result<Self, Self::Error> {
        let meta = payload.expect_meta();
        meta.clone()
            .ok_or(Error::new(ErrorCode::InvalidParams, "Missing metadata"))
            .map(Meta)
    }

    #[inline]
    fn source() -> Source {
        Source::Meta
    }
}

impl RequestArgument for Meta<ProgressToken> {
    type Error = Error;

    #[inline]
    fn extract(payload: Payload<'_>) -> Result<Self, Self::Error> {
        let meta = payload.expect_meta();
        meta.as_ref()
            .and_then(|meta| meta.progress_token.clone())
            .ok_or(Error::new(ErrorCode::InvalidParams, "Missing progress token"))
            .map(Meta)
    }

    #[inline]
    fn source() -> Source {
        Source::Meta
    }
}

impl RequestArgument for Context {
    type Error = Error;

    #[inline]
    fn extract(payload: Payload<'_>) -> Result<Self, Self::Error> {
        let meta = payload.expect_meta();
        meta.as_ref()
            .and_then(|meta| meta.context.clone())
            .ok_or(Error::new(ErrorCode::InvalidParams, "Missing MCP context"))
    }

    #[inline]
    fn source() -> Source {
        Source::Meta
    }
}

#[inline]
pub(crate) fn extract_arg<T: RequestArgument<Error = Error>>(
    meta: &Option<RequestParamsMeta>,
    iter: &mut Iter<'_, String, serde_json::Value>
) -> Result<T, Error> {
    match T::source() {
        Source::Meta => T::extract(Payload::Meta(meta)),
        Source::Args => T::extract(Payload::Args(iter
            .next()
            .ok_or(Error::new(ErrorCode::InvalidParams, "Invalid param provided"))?
            .1.clone())),
    }
}

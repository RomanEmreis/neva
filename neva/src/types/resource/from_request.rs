use std::borrow::Cow;
use std::str::FromStr;
use crate::Context;
use crate::error::{Error, ErrorCode};
use crate::types::{Meta, ProgressToken};
use crate::types::request::RequestParamsMeta;
use super::{Uri, ReadResourceRequestParams};

/// Represents a payload that needs the type to be extracted from
pub(crate) enum Payload<'a> {
    /// Resource URI
    Uri(&'a Uri),
    
    /// Resource URI part
    UriPart(Cow<'a, str>),
    
    /// Request metadata ("_meta")
    Meta(&'a Option<RequestParamsMeta>),
}

/// Represents an extraction sources
pub(crate) enum Source {
    /// Resource URI
    Uri,
    /// Resource URI part
    UriPart,
    /// Request metadata ("_meta")
    Meta,
}

impl<'a> Payload<'a> {
    /// Returns uri part value for type extraction
    #[inline]
    pub(crate) fn expect_uri_part(self) -> Cow<'a, str> {
        match self {
            Payload::UriPart(val) => val,
            _ => unreachable!("Expected UriPart variant"),
        }
    }

    /// Returns a [`Uri`] for type extraction
    #[inline]
    pub(crate) fn expect_uri(self) -> &'a Uri {
        match self {
            Payload::Uri(uri) => uri,
            _ => unreachable!("Expected Uri variant"),
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

/// A trait that type needs to implement to be extractable from [`Request`]
pub(crate) trait ResourceArgument: Sized {
    type Error;

    /// Extracts a type value from [`crate::types::helpers::extract::Payload`]
    fn extract(payload: Payload) -> Result<Self, Self::Error>;

    /// Returns a [`Source`] that the type needs to be extracted from
    #[inline]
    fn source() -> Source {
        Source::UriPart
    }
}

impl TryFrom<ReadResourceRequestParams> for () {
    type Error = Error;

    #[inline]
    fn try_from(_: ReadResourceRequestParams) -> Result<Self, Self::Error> {
        Ok(())
    }
}

impl ResourceArgument for Uri {
    type Error = Error;

    #[inline]
    fn extract(payload: Payload) -> Result<Self, Self::Error> {
        Ok(payload.expect_uri().clone())
    }

    #[inline]
    fn source() -> Source {
        Source::Uri
    }
}

impl<T: FromStr> ResourceArgument for T {
    type Error = Error;

    #[inline]
    fn extract(payload: Payload) -> Result<Self, Self::Error> {
        let part = payload.expect_uri_part();
        part.parse::<T>()
            .map_err(|_| Error::new(ErrorCode::InvalidParams, "Unable to parse URI params"))
    }
}

impl ResourceArgument for Meta<RequestParamsMeta> {
    type Error = Error;

    #[inline]
    fn extract(payload: Payload) -> Result<Self, Self::Error> {
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

impl ResourceArgument for Meta<ProgressToken> {
    type Error = Error;

    #[inline]
    fn extract(payload: Payload) -> Result<Self, Self::Error> {
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

impl ResourceArgument for Context {
    type Error = Error;

    #[inline]
    fn extract(payload: Payload) -> Result<Self, Self::Error> {
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
pub(crate) fn extract_arg<'a, T: ResourceArgument<Error = Error>>(
    uri: &'a Uri,
    meta: &Option<RequestParamsMeta>,
    iter: &mut impl Iterator<Item = Cow<'a, str>>
) -> Result<T, Error> {
    match T::source() {
        Source::Meta => T::extract(Payload::Meta(meta)),
        Source::Uri => T::extract(Payload::Uri(uri)),
        Source::UriPart => T::extract(Payload::UriPart(iter
            .next()
            .ok_or(Error::new(ErrorCode::InvalidParams, "Invalid URI param provided"))?))
    }
}

macro_rules! impl_from_read_resource_params {
    ($($T: ident),*) => {
        impl<$($T: ResourceArgument<Error = Error>),+> TryFrom<ReadResourceRequestParams> for ($($T,)+) {
            type Error = Error;
            
            #[inline]
            fn try_from(params: ReadResourceRequestParams) -> Result<Self, Self::Error> {
                let uri = params.uri;
                let mut iter = params.args.into_iter().flatten();
                let tuple = (
                    $(
                        extract_arg::<$T>(&uri, &params.meta, &mut iter)?,   
                    )*    
                );
                Ok(tuple)
            }
        }
    }
}

impl_from_read_resource_params! { T1 }
impl_from_read_resource_params! { T1, T2 }
impl_from_read_resource_params! { T1, T2, T3 }
impl_from_read_resource_params! { T1, T2, T3, T4 }
impl_from_read_resource_params! { T1, T2, T3, T4, T5 }
use std::str::FromStr;
use crate::error::{Error, ErrorCode};
use super::{Uri, ReadResourceRequestParams};

impl TryFrom<ReadResourceRequestParams> for (Uri,) {
    type Error = Error;

    #[inline]
    fn try_from(params: ReadResourceRequestParams) -> Result<Self, Self::Error> {
        Ok((params.uri,))
    }
}

impl TryFrom<ReadResourceRequestParams> for (ReadResourceRequestParams,) {
    type Error = Error;

    #[inline]
    fn try_from(params: ReadResourceRequestParams) -> Result<Self, Self::Error> {
        Ok((params,))
    }
}

impl TryFrom<ReadResourceRequestParams> for () {
    type Error = Error;

    #[inline]
    fn try_from(_: ReadResourceRequestParams) -> Result<Self, Self::Error> {
        Ok(())
    }
}

macro_rules! impl_from_read_resource_params {
    ($($T: ident),*) => {
        impl<$($T: FromStr),+> TryFrom<ReadResourceRequestParams> for ($($T,)+) {
            type Error = Error;
            
            #[inline]
            fn try_from(params: ReadResourceRequestParams) -> Result<Self, Self::Error> {
                let uri: Uri = params.uri;
                let mut iter = uri.parts()?;
                let tuple = (
                    $(
                    iter.next()
                        .ok_or(Error::new(ErrorCode::InvalidParams, "Invalid URI param provided"))?
                        .parse::<$T>()
                        .map_err(|_| Error::new(ErrorCode::InvalidParams, "Unable to parse URI params"))?,
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
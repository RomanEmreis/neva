use crate::Context;
use crate::types::{Meta, ProgressToken, request::RequestParamsMeta};
use crate::error::{Error, ErrorCode};
use super::GetPromptRequestParams;
use serde::de::DeserializeOwned;

impl TryFrom<GetPromptRequestParams> for () {
    type Error = Error;

    #[inline]
    fn try_from(_: GetPromptRequestParams) -> Result<Self, Self::Error> {
        Ok(())
    }
}

impl TryFrom<GetPromptRequestParams> for (Meta<RequestParamsMeta>,) {
    type Error = Error;

    #[inline]
    fn try_from(params: GetPromptRequestParams) -> Result<Self, Self::Error> {
        params.meta
            .ok_or(Error::new(ErrorCode::InvalidParams, "Missing metadata"))
            .map(|meta| (Meta(meta),))
    }
}

impl TryFrom<GetPromptRequestParams> for (Meta<ProgressToken>,) {
    type Error = Error;

    #[inline]
    fn try_from(params: GetPromptRequestParams) -> Result<Self, Self::Error> {
        params.meta
            .and_then(|meta| meta.progress_token)
            .ok_or(Error::new(ErrorCode::InvalidParams, "Missing progress token"))
            .map(|token| (Meta(token),))
    }
}

impl TryFrom<GetPromptRequestParams> for (Context,) {
    type Error = Error;

    #[inline]
    fn try_from(params: GetPromptRequestParams) -> Result<Self, Self::Error> {
        params.meta
            .and_then(|meta| meta.context)
            .ok_or(Error::new(ErrorCode::InvalidParams, "Missing MCP request context"))
            .map(|ctx| (ctx,))
    }
}

macro_rules! impl_from_get_prompt_params {
    ($($T: ident),*) => {
        impl<$($T: DeserializeOwned),+> TryFrom<GetPromptRequestParams> for ($($T,)+) {
            type Error = Error;
            
            #[inline]
            fn try_from(params: GetPromptRequestParams) -> Result<Self, Self::Error> {
                let args = params.args.ok_or(Error::new(ErrorCode::InvalidParams, "arguments missing"))?;
                let mut iter = args.iter();
                let tuple = (
                    $(
                    $T::deserialize(iter
                            .next()
                            .unwrap().1.clone())?,
                    )*    
                );
                Ok(tuple)
            }
        }
    }
}

impl_from_get_prompt_params! { T1 }
impl_from_get_prompt_params! { T1, T2 }
impl_from_get_prompt_params! { T1, T2, T3 }
impl_from_get_prompt_params! { T1, T2, T3, T4 }
impl_from_get_prompt_params! { T1, T2, T3, T4, T5 }
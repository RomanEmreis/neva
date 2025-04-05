use crate::error::Error;
use super::GetPromptRequestParams;
use serde::{de::DeserializeOwned};

impl TryFrom<GetPromptRequestParams> for () {
    type Error = Error;

    #[inline]
    fn try_from(_: GetPromptRequestParams) -> Result<Self, Self::Error> {
        Ok(())
    }
}

macro_rules! impl_from_get_prompt_params {
    ($($T: ident),*) => {
        impl<$($T: DeserializeOwned),+> TryFrom<GetPromptRequestParams> for ($($T,)+) {
            type Error = Error;
            
            #[inline]
            fn try_from(params: GetPromptRequestParams) -> Result<Self, Self::Error> {
                let args = params.args.ok_or(Error::new("arguments missing"))?;
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
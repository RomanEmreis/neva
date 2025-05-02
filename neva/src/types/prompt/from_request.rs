use crate::error::Error;
use super::GetPromptRequestParams;
use crate::types::helpers::extract::{RequestArgument, extract_arg};

impl TryFrom<GetPromptRequestParams> for () {
    type Error = Error;

    #[inline]
    fn try_from(_: GetPromptRequestParams) -> Result<Self, Self::Error> {
        Ok(())
    }
}

macro_rules! impl_from_get_prompt_params {
    ($($T: ident),*) => {
        impl<$($T: RequestArgument<Error = Error>),+> TryFrom<GetPromptRequestParams> for ($($T,)+) {
            type Error = Error;
            
            #[inline]
            fn try_from(params: GetPromptRequestParams) -> Result<Self, Self::Error> {
                let args = params.args.unwrap_or_default();
                let mut iter = args.iter();
                let tuple = (
                    $(
                        extract_arg::<$T>(&params.meta, &mut iter)?,
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
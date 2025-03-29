use crate::error::Error;
use super::CallToolRequestParams;
use serde::{de::DeserializeOwned};

impl TryFrom<CallToolRequestParams> for () {
    type Error = String;

    #[inline]
    fn try_from(_: CallToolRequestParams) -> Result<Self, Self::Error> {
        Ok(())
    }
}

macro_rules! impl_from_call_tool_params {
    ($($T: ident),*) => {
        impl<$($T: DeserializeOwned),+> TryFrom<CallToolRequestParams> for ($($T,)+) {
            type Error = Error;
            
            #[inline]
            fn try_from(params: CallToolRequestParams) -> Result<Self, Self::Error> {
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

impl_from_call_tool_params! { T1 }
impl_from_call_tool_params! { T1, T2 }
impl_from_call_tool_params! { T1, T2, T3 }
impl_from_call_tool_params! { T1, T2, T3, T4 }
impl_from_call_tool_params! { T1, T2, T3, T4, T5 }
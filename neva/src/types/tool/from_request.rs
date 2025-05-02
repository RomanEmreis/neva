use crate::error::Error;
use super::CallToolRequestParams;
use crate::types::helpers::extract::{RequestArgument, extract_arg};

impl TryFrom<CallToolRequestParams> for () {
    type Error = Error;

    #[inline]
    fn try_from(_: CallToolRequestParams) -> Result<Self, Self::Error> {
        Ok(())
    }
}

macro_rules! impl_from_call_tool_params {
    ($($T: ident),*) => {
        impl<$($T: RequestArgument<Error = Error>),+> TryFrom<CallToolRequestParams> for ($($T,)+) {
            type Error = Error;
            
            #[inline]
            fn try_from(params: CallToolRequestParams) -> Result<Self, Self::Error> {
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
    };
}

impl_from_call_tool_params! { T1 }
impl_from_call_tool_params! { T1, T2 }
impl_from_call_tool_params! { T1, T2, T3 }
impl_from_call_tool_params! { T1, T2, T3, T4 }
impl_from_call_tool_params! { T1, T2, T3, T4, T5 }

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use serde_json::{json, Value};
    use crate::types::{Meta, ProgressToken, request::RequestParamsMeta};
    use super::*;
    
    #[test]
    fn it_extracts_args() {
        let params = CallToolRequestParams {
            args: Some(HashMap::from([
                ("arg".into(), json!({ "test": 1 }))
            ])),
            meta: None,
            name: "tool".into()
        };
        
        let arg: (Value,) = params.try_into().unwrap();
        
        assert_eq!(arg.0, json!({ "test": 1 }));
    }

    #[test]
    #[allow(clippy::useless_conversion)]
    fn it_extracts_params() {
        let params = CallToolRequestParams {
            args: Some(HashMap::from([
                ("arg".into(), json!(22))
            ])),
            meta: None,
            name: "tool".into()
        };

        let arg: CallToolRequestParams = params.try_into().unwrap();

        assert_eq!(arg.name, "tool");
        assert_eq!(arg.args, Some(HashMap::from([
            ("arg".into(), json!(22))
        ])));
    }

    #[test]
    fn it_extracts_meta() {
        let params = CallToolRequestParams {
            name: "tool".into(),
            meta: Some(RequestParamsMeta {
                progress_token: None,
                context: None
            }),
            args: None,
        };

        let arg: (Meta<RequestParamsMeta>,) = params.try_into().unwrap();

        assert_eq!(arg.0.progress_token, None);
    }

    #[test]
    fn it_extracts_progress_token() {
        let params = CallToolRequestParams {
            name: "tool".into(),
            meta: Some(RequestParamsMeta {
                progress_token: Some(ProgressToken::Number(5)),
                context: None
            }),
            args: None,
        };

        let arg: (Meta<ProgressToken>,) = params.try_into().unwrap();

        assert_eq!(arg.0.0, ProgressToken::Number(5));
    }
}
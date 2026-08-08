use super::CallToolRequestParams;
use crate::types::helpers::extract::HandlerArgs;
use crate::types::request::RequestParamsMeta;
use serde_json::Value;
use std::collections::HashMap;

impl HandlerArgs for CallToolRequestParams {
    #[inline]
    fn into_parts(self) -> (Option<HashMap<String, Value>>, Option<RequestParamsMeta>) {
        (self.args, self.meta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ArgNames, FromHandlerArgs, Meta, ProgressToken};
    use serde_json::json;

    fn params(args: Option<HashMap<String, Value>>) -> CallToolRequestParams {
        CallToolRequestParams {
            name: "tool".into(),
            args,
            meta: None,
            #[cfg(feature = "tasks")]
            task: None,
        }
    }

    fn meta(meta: RequestParamsMeta) -> CallToolRequestParams {
        CallToolRequestParams {
            name: "tool".into(),
            args: None,
            meta: Some(meta),
            #[cfg(feature = "tasks")]
            task: None,
        }
    }

    #[test]
    fn it_extracts_a_single_arg() {
        let params = params(Some(HashMap::from([("arg0".into(), json!({ "test": 1 }))])));

        let arg: (Value,) = FromHandlerArgs::from_args(params, &ArgNames::default()).unwrap();

        assert_eq!(arg.0, json!({ "test": 1 }));
    }

    #[test]
    fn it_extracts_args_by_declared_name() {
        let params = params(Some(HashMap::from([
            ("age".into(), json!(30)),
            ("name".into(), json!("John")),
        ])));

        let (name, age): (String, i32) =
            FromHandlerArgs::from_args(params, &ArgNames::new(["name", "age"])).unwrap();

        assert_eq!(name, "John");
        assert_eq!(age, 30);
    }

    #[test]
    fn it_extracts_same_typed_args_without_swapping_them() {
        // The regression this whole path exists for: two arguments of the same
        // type are told apart by name, never by their order in the map.
        let params = params(Some(HashMap::from([
            ("last".into(), json!("Doe")),
            ("first".into(), json!("John")),
        ])));

        let (first, last): (String, String) =
            FromHandlerArgs::from_args(params, &ArgNames::new(["first", "last"])).unwrap();

        assert_eq!(first, "John");
        assert_eq!(last, "Doe");
    }

    #[test]
    fn it_extracts_positional_args_when_no_names_declared() {
        let params = params(Some(HashMap::from([
            ("arg1".into(), json!("John")),
            ("arg0".into(), json!(30)),
        ])));

        let (age, name): (i32, String) =
            FromHandlerArgs::from_args(params, &ArgNames::default()).unwrap();

        assert_eq!(age, 30);
        assert_eq!(name, "John");
    }

    #[test]
    fn it_resolves_an_absent_optional_arg_to_none() {
        let params = params(Some(HashMap::from([("name".into(), json!("John"))])));

        let (name, age): (String, Option<i32>) =
            FromHandlerArgs::from_args(params, &ArgNames::new(["name", "age"])).unwrap();

        assert_eq!(name, "John");
        assert_eq!(age, None);
    }

    #[test]
    fn it_reports_a_missing_arg_by_name() {
        let params = params(Some(HashMap::from([("name".into(), json!("John"))])));

        let err = <(String, i32)>::from_args(params, &ArgNames::new(["name", "age"])).unwrap_err();

        assert_eq!(err.code, crate::error::ErrorCode::InvalidParams);
        assert!(
            err.to_string().contains("missing required argument `age`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn it_reports_a_mistyped_arg_by_name() {
        let params = params(Some(HashMap::from([("age".into(), json!("thirty"))])));

        let err = <(i32,)>::from_args(params, &ArgNames::new(["age"])).unwrap_err();

        assert_eq!(err.code, crate::error::ErrorCode::InvalidParams);
        assert!(
            err.to_string().contains("invalid value for argument `age`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn it_extracts_no_args() {
        let extracted: () = FromHandlerArgs::from_args(params(None), &ArgNames::default()).unwrap();

        assert_eq!(extracted, ());
    }

    #[test]
    fn it_extracts_meta() {
        let params = meta(RequestParamsMeta::default());

        let arg: (Meta<RequestParamsMeta>,) =
            FromHandlerArgs::from_args(params, &ArgNames::default()).unwrap();

        assert_eq!(arg.0.progress_token, None);
    }

    #[test]
    fn it_extracts_progress_token() {
        let params = meta(RequestParamsMeta {
            progress_token: Some(ProgressToken::Number(5)),
            ..Default::default()
        });

        let arg: (Meta<ProgressToken>,) =
            FromHandlerArgs::from_args(params, &ArgNames::default()).unwrap();

        assert_eq!(arg.0.0, ProgressToken::Number(5));
    }

    #[test]
    fn it_does_not_let_a_meta_arg_consume_an_argument_slot() {
        // `Meta<_>` comes from `_meta`, so the `String` after it still reads
        // the *first* declared argument name.
        let mut params = params(Some(HashMap::from([("name".into(), json!("John"))])));
        params.meta = Some(RequestParamsMeta {
            progress_token: Some(ProgressToken::Number(5)),
            ..Default::default()
        });

        let (token, name): (Meta<ProgressToken>, String) =
            FromHandlerArgs::from_args(params, &ArgNames::new(["name"])).unwrap();

        assert_eq!(token.0, ProgressToken::Number(5));
        assert_eq!(name, "John");
    }
}

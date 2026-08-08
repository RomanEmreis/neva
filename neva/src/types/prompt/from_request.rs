use super::GetPromptRequestParams;
use crate::types::helpers::extract::HandlerArgs;
use crate::types::request::RequestParamsMeta;
use serde_json::Value;
use std::collections::HashMap;

impl HandlerArgs for GetPromptRequestParams {
    #[inline]
    fn into_parts(self) -> (Option<HashMap<String, Value>>, Option<RequestParamsMeta>) {
        (self.args, self.meta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ArgNames, FromHandlerArgs};
    use serde_json::json;

    #[test]
    fn it_publishes_an_optional_arg_as_not_required() {
        use crate::types::prompt::Prompt;

        let prompt = Prompt::new(
            "analyze",
            |topic: String, tone: Option<String>| async move {
                (format!("{topic}/{tone:?}"), crate::types::Role::User)
            },
        );
        let args = prompt.args.as_ref().unwrap();

        assert_eq!(args.len(), 2);
        assert_eq!(args[0].required, Some(true));
        assert_eq!(args[1].required, Some(false));
    }

    #[test]
    fn it_resolves_an_absent_optional_arg_to_none() {
        let params = GetPromptRequestParams {
            name: "analyze".into(),
            args: Some(HashMap::from([("topic".into(), json!("rust"))])),
            meta: None,
        };

        let (topic, tone): (String, Option<String>) =
            FromHandlerArgs::from_args(params, &ArgNames::new(["topic", "tone"])).unwrap();

        assert_eq!(topic, "rust");
        assert_eq!(tone, None);
    }

    #[test]
    fn it_extracts_args_by_declared_name() {
        let params = GetPromptRequestParams {
            name: "prompt".into(),
            args: Some(HashMap::from([
                ("tone".into(), json!("formal")),
                ("topic".into(), json!("rust")),
            ])),
            meta: None,
        };

        let (topic, tone): (String, String) =
            FromHandlerArgs::from_args(params, &ArgNames::new(["topic", "tone"])).unwrap();

        assert_eq!(topic, "rust");
        assert_eq!(tone, "formal");
    }
}

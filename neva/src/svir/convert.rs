//! Between MCP's tool types and svir's: what a model is offered, what it
//! sends back, and what it is told.

use crate::error::{Error, ErrorCode};
use crate::types::{CallToolResponse, Content, ResourceContents, Tool};
use ::svir::{ToolCall, ToolResult};
use serde_json::Value;
use std::collections::HashMap;

/// The longest function name the model APIs accept.
const MAX_NAME_LEN: usize = 64;

/// What separates the parts of a result: svir joins text the same way (P12).
const SEPARATOR: &str = "\n\n";

/// `tool` as a model is offered it, under `name`.
///
/// Fails for a name a model API cannot carry. Function names are
/// `[a-zA-Z0-9_-]{1,64}`, where MCP also allows `.` and up to 128
/// characters; rewriting one would have the model call a tool the server does
/// not know by that name, and the caller would not know why.
pub(super) fn descriptor(tool: &Tool, name: String) -> Result<::svir::Tool, Error> {
    let carried = name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');

    if !carried || name.is_empty() || name.len() > MAX_NAME_LEN {
        return Err(Error::new(
            ErrorCode::InvalidParams,
            format!(
                "Tool `{name}` cannot be offered to a model: a function name is 1 to \
                 {MAX_NAME_LEN} of `a-z`, `A-Z`, `0-9`, `_` and `-`"
            ),
        ));
    }

    let description = tool
        .descr
        .as_ref()
        .or(tool.title.as_ref())
        .cloned()
        .unwrap_or_default();

    let schema = serde_json::to_value(&tool.input_schema)?;

    Ok(::svir::Tool::new(name, description).schema(schema))
}

/// The arguments of `call`, as `tools/call` takes them.
///
/// A model sends a call without arguments as an empty string or `null` as
/// often as `{}`, so all three are no arguments. Anything else that is not a
/// JSON object is the model's mistake, and is told back to it.
pub(super) fn arguments(call: &ToolCall) -> Result<HashMap<String, Value>, String> {
    let raw = call.arguments.trim();
    if raw.is_empty() {
        return Ok(HashMap::new());
    }

    match serde_json::from_str(raw) {
        Ok(Value::Object(args)) => Ok(args.into_iter().collect()),
        Ok(Value::Null) => Ok(HashMap::new()),
        Ok(_) => Err(format!(
            "The arguments of `{}` must be a JSON object",
            call.name
        )),
        Err(err) => Err(format!(
            "The arguments of `{}` are not valid JSON: {err}",
            call.name
        )),
    }
}

/// What the model is told of a `tools/call`, answering call `call_id`.
///
/// A tool result is a string, so the content is flattened into one:
///
/// - text blocks are joined with a blank line;
/// - structured content is sent as JSON text, but only without text blocks:
///   a tool returning structured content is to return it serialized in a text
///   block too, and taking both would show the model the same data twice;
/// - an embedded text resource is its text, a JSON one its JSON, and a
///   resource link its name and URI.
///
/// Images, audio and binary resources answer an error naming what could not
/// be passed on, rather than being dropped. A result the tool flagged as an
/// error, and a call the server refused, answer an error with its message.
pub(super) fn result(call_id: &str, outcome: Result<CallToolResponse, Error>) -> ToolResult {
    let response = match outcome {
        Ok(response) => response,
        Err(err) => return ToolResult::error(call_id, err.to_string()),
    };

    match flatten(&response) {
        Ok(text) if response.is_error => ToolResult::error(call_id, text),
        Ok(text) => ToolResult::new(call_id, text),
        Err(err) => ToolResult::error(call_id, err),
    }
}

fn flatten(response: &CallToolResponse) -> Result<String, String> {
    let mut parts = Vec::with_capacity(response.content.len());
    let mut has_text = false;

    for block in &response.content {
        match block {
            Content::Text(text) => {
                has_text = true;
                parts.push(text.text.clone());
            }
            Content::ResourceLink(link) => parts.push(format!("{}: {}", link.name, link.uri)),
            Content::Resource(embedded) => match &embedded.resource {
                ResourceContents::Text(resource) => parts.push(resource.text.clone()),
                ResourceContents::Json(resource) => parts.push(resource.value.to_string()),
                ResourceContents::Blob(resource) => {
                    return Err(unsupported(&format!(
                        "a binary resource (`{}`)",
                        resource.uri
                    )));
                }
                ResourceContents::Empty(_) => {}
            },
            Content::Image(_) => return Err(unsupported("an image")),
            Content::Audio(_) => return Err(unsupported("audio")),
            Content::ToolUse(_) | Content::ToolResult(_) => {
                return Err(unsupported("sampling content"));
            }
            Content::Empty(_) => {}
        }
    }

    if !has_text && let Some(structured) = &response.struct_content {
        parts.push(structured.to_string());
    }

    Ok(parts.join(SEPARATOR))
}

fn unsupported(what: &str) -> String {
    format!("The tool returned {what}, which cannot be passed on to the model")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ResourceLink, TextResourceContents};
    use serde_json::json;

    fn tool(name: &str) -> Tool {
        serde_json::from_value(json!({
            "name": name,
            "description": "Adds two numbers.",
            "inputSchema": {
                "type": "object",
                "properties": { "a": { "type": "integer" }, "b": { "type": "integer" } }
            }
        }))
        .expect("a tool")
    }

    fn call(arguments: &str) -> ToolCall {
        ToolCall::new("call-1", "add", arguments)
    }

    fn response(content: Vec<Content>) -> CallToolResponse {
        CallToolResponse {
            content,
            struct_content: None,
            is_error: false,
        }
    }

    #[test]
    fn a_descriptor_carries_name_description_and_schema() {
        let offered = descriptor(&tool("add"), "math_add".into()).expect("a descriptor");

        assert_eq!(offered.name, "math_add");
        assert_eq!(offered.description, "Adds two numbers.");
        assert_eq!(offered.input_schema["properties"]["a"]["type"], "integer");
    }

    #[test]
    fn a_tool_without_a_description_offers_its_title() {
        let mut titled = tool("add");
        titled.descr = None;
        titled.title = Some("Addition".into());

        let offered = descriptor(&titled, "add".into()).expect("a descriptor");
        assert_eq!(offered.description, "Addition");
    }

    /// MCP allows names a model API cannot carry. One is refused, not
    /// rewritten into a name the server does not know.
    #[test]
    fn a_name_a_model_cannot_carry_is_refused() {
        for name in ["files.read", "has space", "", &"x".repeat(MAX_NAME_LEN + 1)] {
            assert!(
                descriptor(&tool("add"), name.to_string()).is_err(),
                "{name:?} must be refused"
            );
        }
        assert!(descriptor(&tool("add"), "x".repeat(MAX_NAME_LEN)).is_ok());
    }

    #[test]
    fn arguments_are_an_object() {
        let args = arguments(&call(r#"{"a": 1, "b": 2}"#)).expect("an object");
        assert_eq!(args["a"], 1);
        assert_eq!(args["b"], 2);
    }

    #[test]
    fn no_arguments_is_an_empty_object() {
        for raw in ["", "  ", "null", "{}"] {
            assert!(
                arguments(&call(raw)).expect("no arguments").is_empty(),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn arguments_that_are_not_an_object_are_told_back() {
        for raw in ["[1, 2]", "42", r#""a""#, "{not json"] {
            let err = arguments(&call(raw)).expect_err("refused");
            assert!(err.contains("`add`"), "{err}");
        }
    }

    #[test]
    fn text_blocks_are_joined_with_a_blank_line() {
        let told = result(
            "call-1",
            Ok(response(vec![Content::text("one"), Content::text("two")])),
        );

        assert_eq!(told.call_id, "call-1");
        assert_eq!(told.content, "one\n\ntwo");
        assert!(!told.is_error);
    }

    /// A tool returning structured content returns it as text too; both would
    /// show the model the same data twice.
    #[test]
    fn structured_content_is_sent_only_without_text() {
        let mut both = response(vec![Content::text(r#"{"sum":3}"#)]);
        both.struct_content = Some(json!({ "sum": 3 }));
        assert_eq!(result("c", Ok(both)).content, r#"{"sum":3}"#);

        let mut structured = response(Vec::new());
        structured.struct_content = Some(json!({ "sum": 3 }));
        assert_eq!(result("c", Ok(structured)).content, r#"{"sum":3}"#);
    }

    #[test]
    fn resources_are_their_text_and_links_their_uri() {
        let text = TextResourceContents::new("file:///notes.md", "# Notes");
        let link: ResourceLink = serde_json::from_value(json!({
            "type": "resource_link",
            "uri": "file:///report.pdf",
            "name": "report"
        }))
        .expect("a link");

        let told = result(
            "c",
            Ok(response(vec![Content::resource(text), Content::from(link)])),
        );
        assert_eq!(told.content, "# Notes\n\nreport: file:///report.pdf");
    }

    /// Dropping an image would leave the model answering from part of the
    /// result without knowing it; the error says what was not passed on.
    #[test]
    fn media_is_an_error_not_dropped() {
        for content in [Content::image(vec![1, 2, 3]), Content::audio(vec![1, 2, 3])] {
            let told = result("c", Ok(response(vec![Content::text("caption"), content])));
            assert!(told.is_error);
            assert!(
                told.content.contains("cannot be passed on"),
                "{}",
                told.content
            );
        }
    }

    #[test]
    fn a_failed_tool_or_a_refused_call_is_an_error() {
        let mut failed = response(vec![Content::text("division by zero")]);
        failed.is_error = true;
        let told = result("c", Ok(failed));
        assert!(told.is_error);
        assert_eq!(told.content, "division by zero");

        let refused = Error::new(ErrorCode::InvalidParams, "Tool not found");
        let told = result("c", Err(refused));
        assert!(told.is_error);
        assert_eq!(told.content, "Tool not found");
    }
}

//! Between MCP's types and svir's: the tools a model is offered, what it
//! sends back and is told, and the prompts and resources a conversation is
//! given.

use crate::error::{Error, ErrorCode};
use crate::types::{
    CallToolResponse, Content, GetPromptResult, ReadResourceResult, ResourceContents, Role, Tool,
    ToolInputSchema,
};
use ::svir::{Image, Message, Part, TextFile, ToolCall, ToolResult};
use serde_json::Value;
use std::collections::HashMap;

/// The longest function name the model APIs accept.
const MAX_NAME_LEN: usize = 64;

/// What separates the parts of a result: svir joins text the same way (P12).
const SEPARATOR: &str = "\n\n";

/// `tool` as a model is offered it, under `name`; the tool's own name is not
/// read.
///
/// Fails for a name a model API cannot carry. Function names are
/// `[a-zA-Z0-9_-]{1,64}`, where MCP also allows `.` and up to 128
/// characters; rewriting one would have the model call a tool the server does
/// not know by that name, and the caller would not know why.
pub(super) fn descriptor(tool: Tool, name: String) -> Result<::svir::Tool, Error> {
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

    let description = tool.descr.or(tool.title).unwrap_or_default();
    let schema = schema(tool.input_schema)?;

    Ok(::svir::Tool::new(name, description).schema(schema))
}

/// The input schema as JSON. Under MCP 2026-07-28 it already is JSON, and is
/// moved rather than serialized into a copy.
#[cfg(not(feature = "legacy-spec"))]
#[inline]
fn schema(schema: ToolInputSchema) -> Result<Value, Error> {
    Ok(schema.into_value())
}

#[cfg(feature = "legacy-spec")]
#[inline]
fn schema(schema: ToolInputSchema) -> Result<Value, Error> {
    Ok(serde_json::to_value(schema)?)
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

    // Straight into the map `tools/call` takes: valid JSON of another shape
    // is a data error, anything else is not JSON at all.
    match serde_json::from_str::<Option<HashMap<String, Value>>>(raw) {
        Ok(args) => Ok(args.unwrap_or_default()),
        Err(err) if err.is_data() => Err(format!(
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
pub(super) fn result(
    call_id: impl Into<String>,
    outcome: Result<CallToolResponse, Error>,
) -> ToolResult {
    let response = match outcome {
        Ok(response) => response,
        Err(err) => return ToolResult::error(call_id, err.to_string()),
    };

    let is_error = response.is_error;
    match flatten(response) {
        Ok(text) if is_error => ToolResult::error(call_id, text),
        Ok(text) => ToolResult::new(call_id, text),
        Err(err) => ToolResult::error(call_id, err),
    }
}

/// The response as one string, its text moved rather than copied: a tool
/// answering with one text block is told it as it is.
fn flatten(response: CallToolResponse) -> Result<String, String> {
    let mut told = None;
    let mut has_text = false;

    for block in response.content {
        match block {
            Content::Text(text) => {
                has_text = true;
                join(&mut told, text.text);
            }
            Content::ResourceLink(link) => join(&mut told, format!("{}: {}", link.name, link.uri)),
            Content::Resource(embedded) => match embedded.resource {
                ResourceContents::Text(resource) => join(&mut told, resource.text),
                ResourceContents::Json(resource) => join(&mut told, resource.value.to_string()),
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

    if !has_text && let Some(structured) = response.struct_content {
        join(&mut told, structured.to_string());
    }

    Ok(told.unwrap_or_default())
}

/// Adds `part` to what the model is told, after a blank line.
fn join(told: &mut Option<String>, part: String) {
    match told {
        Some(told) => {
            told.push_str(SEPARATOR);
            told.push_str(&part);
        }
        None => *told = Some(part),
    }
}

fn unsupported(what: &str) -> String {
    format!("The tool returned {what}, which cannot be passed on to the model")
}

/// A prompt (`prompts/get`) as the messages it opens a conversation with.
///
/// Roles carry over, and consecutive messages of one role become one message:
/// MCP gives each content block a message of its own, where a turn of a
/// conversation is a role and its parts. Text is text, an image an image, an
/// embedded resource is attached as [`resource_parts`] attaches it, and a
/// resource link is its name and URI. Audio, binary resources other than
/// images, and sampling content cannot be passed on, and are an error rather
/// than dropped. The prompt's description is for the user, not the model,
/// and is left out.
///
/// # Examples
/// ```no_run
/// use neva::{Client, error::Error};
///
/// # async fn run(client: Client) -> Result<(), Error> {
/// let prompt = client.prompts().get("review", [("lang", "rust")]).await?;
/// let messages = neva::svir::prompt_messages(prompt)?;
///
/// // The conversation so far, before the model answers: each one goes to
/// // `svir::Request::message`.
/// # Ok(())
/// # }
/// ```
pub fn prompt_messages(prompt: GetPromptResult) -> Result<Vec<Message>, Error> {
    let mut messages: Vec<Message> = Vec::with_capacity(prompt.messages.len());

    for message in prompt.messages {
        let role = match message.role {
            Role::User => ::svir::Role::User,
            Role::Assistant => ::svir::Role::Assistant,
        };
        let Some(part) = content_part(message.content)? else {
            continue;
        };

        match messages.last_mut() {
            Some(turn) if turn.role == role => turn.parts.push(part),
            _ => messages.push(Message::new(role).with(part)),
        }
    }

    Ok(messages)
}

/// A resource (`resources/read`) as the parts to attach to a message.
///
/// Text contents are a text file named by the resource's URI, which is how
/// the model is told where they came from; JSON contents likewise, as JSON
/// text; an image is an image. Other binary contents cannot be passed on, and
/// are an error rather than dropped.
///
/// # Examples
/// ```no_run
/// use neva::{Client, error::Error};
///
/// # async fn run(client: Client) -> Result<(), Error> {
/// let notes = client.resources().read("file:///notes.md").await?;
/// let parts = neva::svir::resource_parts(notes)?;
///
/// // Attached to a question: each one goes to `svir::Message::with`, after
/// // `svir::Message::user("Summarise these notes.")`.
/// # Ok(())
/// # }
/// ```
pub fn resource_parts(resource: ReadResourceResult) -> Result<Vec<Part>, Error> {
    resource
        .contents
        .into_iter()
        .filter_map(|contents| resource_part(contents).transpose())
        .collect()
}

/// One content block of a prompt or a sampling message, or `None` for an
/// empty one.
pub(super) fn content_part(content: Content) -> Result<Option<Part>, Error> {
    match content {
        Content::Text(text) => Ok(Some(Part::Text(text.text))),
        Content::Image(image) => Ok(Some(Image::bytes(image.data, image.mime).into())),
        Content::Resource(embedded) => resource_part(embedded.resource),
        Content::ResourceLink(link) => Ok(Some(Part::Text(format!("{}: {}", link.name, link.uri)))),
        Content::Audio(_) => Err(refused("Audio")),
        Content::ToolUse(_) | Content::ToolResult(_) => Err(refused("Sampling content")),
        Content::Empty(_) => Ok(None),
    }
}

/// One content of a resource, or `None` for an empty one.
fn resource_part(contents: ResourceContents) -> Result<Option<Part>, Error> {
    match contents {
        ResourceContents::Text(text) => {
            Ok(Some(TextFile::text(text.uri.to_string(), text.text).into()))
        }
        ResourceContents::Json(json) => Ok(Some(
            TextFile::text(json.uri.to_string(), json.value.to_string()).into(),
        )),
        ResourceContents::Blob(blob) => match blob.mime {
            Some(mime) if mime.starts_with("image/") => {
                Ok(Some(Image::bytes(blob.blob, mime).into()))
            }
            mime => Err(refused(&format!(
                "The binary resource `{}` ({})",
                blob.uri,
                mime.as_deref().unwrap_or("no media type")
            ))),
        },
        ResourceContents::Empty(_) => Ok(None),
    }
}

pub(super) fn refused(what: &str) -> Error {
    Error::new(
        ErrorCode::InvalidParams,
        format!("{what} cannot be passed on to the model"),
    )
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
        let offered = descriptor(tool("add"), "math_add".into()).expect("a descriptor");

        assert_eq!(offered.name, "math_add");
        assert_eq!(offered.description, "Adds two numbers.");
        assert_eq!(offered.input_schema["properties"]["a"]["type"], "integer");
    }

    #[test]
    fn a_tool_without_a_description_offers_its_title() {
        let mut titled = tool("add");
        titled.descr = None;
        titled.title = Some("Addition".into());

        let offered = descriptor(titled, "add".into()).expect("a descriptor");
        assert_eq!(offered.description, "Addition");
    }

    /// MCP allows names a model API cannot carry. One is refused, not
    /// rewritten into a name the server does not know.
    #[test]
    fn a_name_a_model_cannot_carry_is_refused() {
        for name in ["files.read", "has space", "", &"x".repeat(MAX_NAME_LEN + 1)] {
            assert!(
                descriptor(tool("add"), name.to_string()).is_err(),
                "{name:?} must be refused"
            );
        }
        assert!(descriptor(tool("add"), "x".repeat(MAX_NAME_LEN)).is_ok());
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

    fn prompt(messages: serde_json::Value) -> GetPromptResult {
        serde_json::from_value(json!({ "messages": messages })).expect("a prompt")
    }

    fn resource(contents: serde_json::Value) -> ReadResourceResult {
        serde_json::from_value(json!({ "contents": contents, "ttlMs": 0 })).expect("a resource")
    }

    /// MCP gives each content block a message of its own; a turn of a
    /// conversation is a role and its parts.
    #[test]
    fn a_prompt_is_its_turns() {
        let messages = prompt_messages(prompt(json!([
            { "role": "user", "content": { "type": "text", "text": "What is on this chart?" } },
            { "role": "user", "content": { "type": "image", "data": "AQID", "mimeType": "image/png" } },
            { "role": "assistant", "content": { "type": "text", "text": "A rising line." } },
            { "role": "user", "content": { "type": "text", "text": "Why?" } }
        ])))
        .expect("messages");

        let roles: Vec<_> = messages.iter().map(|message| message.role).collect();
        assert_eq!(
            roles,
            [
                ::svir::Role::User,
                ::svir::Role::Assistant,
                ::svir::Role::User
            ]
        );

        let first = &messages[0].parts;
        assert_eq!(first.len(), 2);
        assert_eq!(first[0], Part::Text("What is on this chart?".into()));
        assert_eq!(
            first[1],
            Part::Image(Image::bytes(vec![1, 2, 3], "image/png"))
        );
    }

    #[test]
    fn a_prompt_attaches_its_resources() {
        let messages = prompt_messages(prompt(json!([
            {
                "role": "user",
                "content": {
                    "type": "resource",
                    "resource": { "uri": "file:///notes.md", "text": "# Notes" }
                }
            },
            {
                "role": "user",
                "content": { "type": "resource_link", "uri": "file:///report.pdf", "name": "report" }
            }
        ])))
        .expect("messages");

        assert_eq!(
            messages[0].parts,
            [
                Part::File(TextFile::text("file:///notes.md", "# Notes")),
                Part::Text("report: file:///report.pdf".into())
            ]
        );
    }

    /// Dropping a block would leave the model reading part of the prompt
    /// without knowing it.
    #[test]
    fn a_prompt_with_audio_is_an_error() {
        let err = prompt_messages(prompt(json!([
            { "role": "user", "content": { "type": "audio", "data": "AQID", "mimeType": "audio/wav" } }
        ])))
        .expect_err("audio cannot be passed on");
        assert!(err.to_string().contains("Audio"), "{err}");
    }

    #[test]
    fn a_resource_is_files_and_images() {
        let mut read = resource(json!([
            { "uri": "file:///notes.md", "text": "# Notes" },
            { "uri": "file:///chart.png", "blob": "AQID", "mimeType": "image/png" }
        ]));
        read.contents.push(ResourceContents::Json(
            crate::types::resource::JsonResourceContents::new(
                "file:///data.json",
                json!({ "a": 1 }),
            ),
        ));

        assert_eq!(
            resource_parts(read).expect("parts"),
            [
                Part::File(TextFile::text("file:///notes.md", "# Notes")),
                Part::Image(Image::bytes(vec![1, 2, 3], "image/png")),
                Part::File(TextFile::text("file:///data.json", r#"{"a":1}"#)),
            ]
        );
    }

    #[test]
    fn a_binary_resource_that_is_not_an_image_is_an_error() {
        let err = resource_parts(resource(json!([
            { "uri": "file:///report.pdf", "blob": "AQID", "mimeType": "application/pdf" }
        ])))
        .expect_err("a PDF cannot be passed on");
        assert!(err.to_string().contains("file:///report.pdf"), "{err}");
        assert!(err.to_string().contains("application/pdf"), "{err}");
    }
}

//! Sampling through svir: what a server asks a model in
//! `sampling/createMessage` as a request to one, and the model's answer as
//! the sample the server gets back.

use super::convert::{self, refused};
use crate::error::{Error, ErrorCode};
use crate::types::sampling::{
    CreateMessageRequestParams, CreateMessageResult, SamplingMessage, StopReason, ToolChoiceMode,
};
use crate::types::{CallToolResponse, Content, Role, ToolUse};
use ::svir::{Completion, FinishReason, Message, Part, Request, ToolCall, ToolChoice};
use serde_json::Value;
use std::collections::BTreeMap;

/// A server's `sampling/createMessage` as a request to `model`.
///
/// The system prompt, the messages, the tools, `maxTokens` and `temperature`
/// carry over:
///
/// - roles are kept, and consecutive content of one role is one turn;
/// - a `tool_use` block is a call in the assistant's turn, and each
///   `tool_result` a tool message of its own, flattened as a tool's answer
///   is for a model: text joined, and an image, audio or a binary resource
///   an error the model is told of;
/// - text, images and resources are as in [`prompt_messages`](super::prompt_messages);
/// - `toolChoice: none` leaves the tools out, and `toolChoice: required` asks
///   the model for a call, which needs tools to call. A model server may take
///   it and answer without one; the sample then says so with `endTurn`.
///
/// Audio and stop sequences cannot be passed on, and are an error rather than
/// dropped. What the spec leaves to the client is
/// left out: `includeContext`, `modelPreferences`, since the model is the
/// caller's choice, and the provider `metadata`.
///
/// # Examples
/// ```no_run
/// use neva::{Client, types::sampling::CreateMessageRequestParams};
///
/// # async fn complete(_: &svir::Request) -> Result<svir::Completion, svir::Error> {
/// #     unimplemented!()
/// # }
/// let mut client = Client::new();
///
/// #[allow(deprecated)]
/// client.map_sampling(|params: CreateMessageRequestParams| async move {
///     let request = neva::svir::sampling_request("qwen3", params)?;
///     // `svir::Client::complete`, with svir's `client` feature on.
///     let done = complete(&request).await?;
///     neva::svir::sampling_result(&request, done)
/// });
/// ```
pub fn sampling_request(
    model: impl Into<String>,
    params: CreateMessageRequestParams,
) -> Result<Request, Error> {
    if params
        .stop_sequences
        .as_ref()
        .is_some_and(|stops| !stops.is_empty())
    {
        return Err(refused("Stop sequences"));
    }
    let max_tokens = u64::try_from(params.max_tokens)
        .ok()
        .filter(|tokens| *tokens > 0)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidParams,
                format!("`maxTokens` must be positive, not {}", params.max_tokens),
            )
        })?;

    let mut request = Request::new(model).max_tokens(max_tokens);
    if let Some(system) = params.sys_prompt {
        request = request.system(system);
    }
    if let Some(temperature) = params.temp {
        request = request.temperature(temperature);
    }

    let choice = params.tool_choice.map(|choice| choice.mode);
    if choice != Some(ToolChoiceMode::None) {
        for mut tool in params.tools.into_iter().flatten() {
            let name = std::mem::take(&mut tool.name);
            request = request.tool(convert::descriptor(tool, name)?);
        }
    }
    if choice == Some(ToolChoiceMode::Required) {
        if request.tools.is_empty() {
            return Err(Error::new(
                ErrorCode::InvalidParams,
                "`toolChoice: required` asks for a tool call, and no tools were given",
            ));
        }
        request = request.tool_choice(ToolChoice::Required);
    }

    Ok(turns(params.messages)?
        .into_iter()
        .fold(request, Request::message))
}

/// The model's answer to `request` as the sample the server gets back.
///
/// Its text, then a `tool_use` block per call, each keeping the call's id so
/// that the `tool_result` the server sends back finds it. The stop reason is
/// `toolUse` when the model called a tool, and otherwise follows why it
/// stopped: `endTurn`, `maxTokens`, `contentFilter` when the model server's
/// filter stopped the answer, or `refusal` when the model refused, the text
/// then its refusal. The model is the one `request` asked
/// for. Reasoning and token counts have no place in a sample and are left out.
///
/// A call whose arguments are not a JSON object is an error: the server could
/// not run it.
///
/// # Examples
/// ```
/// use neva::types::sampling::StopReason;
///
/// let request = svir::Request::new("qwen3").user("What is 2 + 3?");
/// let mut done = svir::Completion::new(svir::FinishReason::Stop);
/// done.text = "5".into();
///
/// let sample = neva::svir::sampling_result(&request, done)?;
/// assert_eq!(sample.model, "qwen3");
/// assert_eq!(sample.stop_reason, Some(StopReason::EndTurn));
/// # Ok::<(), neva::error::Error>(())
/// ```
pub fn sampling_result(request: &Request, done: Completion) -> Result<CreateMessageResult, Error> {
    let stop = if done.calls.is_empty() {
        stop_reason(done.finish)
    } else {
        StopReason::ToolUse
    };
    let mut sample = CreateMessageResult::assistant()
        .with_model(request.model.clone())
        .with_stop_reason(stop);

    if !done.text.is_empty() || done.calls.is_empty() {
        sample = sample.with_content(Content::text(done.text));
    }
    for call in done.calls {
        let input =
            convert::arguments(&call).map_err(|err| Error::new(ErrorCode::InternalError, err))?;
        sample = sample.with_content(ToolUse {
            id: call.id,
            name: call.name,
            input: Some(input),
            meta: None,
        });
    }

    Ok(sample)
}

/// The turns of a sampled conversation.
///
/// MCP puts a tool's result in a user message, where a model API gives each
/// result a message of its own, so a `tool_result` is never merged into a
/// turn.
fn turns(messages: Vec<SamplingMessage>) -> Result<Vec<Message>, Error> {
    let mut turns: Vec<Message> = Vec::with_capacity(messages.len());

    for message in messages {
        for content in message.content {
            let (role, part) = match (message.role, content) {
                (Role::Assistant, Content::ToolUse(call)) => {
                    (::svir::Role::Assistant, Part::ToolCall(tool_call(call)?))
                }
                // The answer of a `tools/call` the server ran, told to the
                // model as `RemoteTools` tells it one.
                (Role::User, Content::ToolResult(result)) => {
                    let response = CallToolResponse {
                        content: result.content,
                        struct_content: result.struct_content,
                        is_error: result.is_error,
                    };
                    let told = convert::result(result.tool_use_id, Ok(response));
                    turns.push(Message::tool_result(told));
                    continue;
                }
                (Role::User, Content::ToolUse(_)) => {
                    return Err(misplaced(
                        "A `tool_use` block in a user message: a tool call is the assistant's",
                    ));
                }
                (Role::Assistant, Content::ToolResult(_)) => {
                    return Err(misplaced(
                        "A `tool_result` block in an assistant message: a tool result is the user's",
                    ));
                }
                (role, content) => match convert::content_part(content)? {
                    Some(part) => (svir_role(role), part),
                    None => continue,
                },
            };

            match turns.last_mut() {
                Some(turn) if turn.role == role => turn.parts.push(part),
                _ => turns.push(Message::new(role).with(part)),
            }
        }
    }

    Ok(turns)
}

/// A `tool_use` block as the call the model made, its input as JSON text.
///
/// The keys go out sorted: a `HashMap` has no order of its own, and a model
/// server caching a conversation's prefix finds it again only when a call is
/// written the same way every round.
fn tool_call(call: ToolUse) -> Result<ToolCall, Error> {
    let input: BTreeMap<&String, &Value> = call.input.iter().flatten().collect();
    let arguments = serde_json::to_string(&input)?;
    Ok(ToolCall::new(call.id, call.name, arguments))
}

fn svir_role(role: Role) -> ::svir::Role {
    match role {
        Role::User => ::svir::Role::User,
        Role::Assistant => ::svir::Role::Assistant,
    }
}

fn stop_reason(finish: FinishReason) -> StopReason {
    match finish {
        FinishReason::Length => StopReason::MaxTokens,
        FinishReason::ContentFilter => StopReason::Other("contentFilter".into()),
        FinishReason::Refusal => StopReason::Other("refusal".into()),
        // `Stop`, `ToolCalls` without a call, and whatever svir adds later:
        // the answer ended.
        _ => StopReason::EndTurn,
    }
}

fn misplaced(what: &'static str) -> Error {
    Error::new(ErrorCode::InvalidParams, what)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::svir::{Image, ToolResult};
    use serde_json::json;

    fn params(value: Value) -> CreateMessageRequestParams {
        serde_json::from_value(value).expect("sampling params")
    }

    fn add_tool() -> Value {
        json!({
            "name": "add",
            "description": "Adds two numbers.",
            "inputSchema": {
                "type": "object",
                "properties": { "a": { "type": "integer" }, "b": { "type": "integer" } }
            }
        })
    }

    #[test]
    fn a_request_carries_the_prompt_the_tools_and_the_limits() {
        let request = sampling_request(
            "qwen3",
            params(json!({
                "systemPrompt": "You add numbers.",
                "messages": [{ "role": "user", "content": { "type": "text", "text": "2 + 3?" } }],
                "maxTokens": 100,
                "temperature": 0.2,
                "tools": [add_tool()]
            })),
        )
        .expect("a request");

        assert_eq!(request.model, "qwen3");
        assert_eq!(request.system.as_deref(), Some("You add numbers."));
        assert_eq!(request.max_tokens, Some(100));
        assert_eq!(request.temperature, Some(0.2));
        assert_eq!(request.tools.len(), 1);
        assert_eq!(request.tools[0].name, "add");
        assert_eq!(request.messages, [Message::user("2 + 3?")]);
    }

    /// The round a server sends after running the model's call: the call in
    /// the assistant's turn, its result a tool message of its own.
    #[test]
    fn a_tool_round_is_a_call_and_its_result() {
        let request = sampling_request(
            "qwen3",
            params(json!({
                "messages": [
                    { "role": "user", "content": { "type": "text", "text": "2 + 3?" } },
                    {
                        "role": "assistant",
                        "content": [
                            { "type": "text", "text": "Adding." },
                            { "type": "tool_use", "id": "call-1", "name": "add", "input": { "b": 3, "a": 2 } }
                        ]
                    },
                    {
                        "role": "user",
                        "content": [
                            {
                                "type": "tool_result",
                                "toolUseId": "call-1",
                                "content": [{ "type": "text", "text": "5" }]
                            },
                            {
                                "type": "tool_result",
                                "toolUseId": "call-2",
                                "content": [{ "type": "text", "text": "no such tool" }],
                                "isError": true
                            }
                        ]
                    },
                    { "role": "user", "content": { "type": "text", "text": "Thanks." } }
                ],
                "maxTokens": 100
            })),
        )
        .expect("a request");

        assert_eq!(
            request.messages,
            [
                Message::user("2 + 3?"),
                Message::assistant("Adding.").with(ToolCall::new(
                    "call-1",
                    "add",
                    r#"{"a":2,"b":3}"#
                )),
                Message::tool_result(ToolResult::new("call-1", "5")),
                Message::tool_result(ToolResult::error("call-2", "no such tool")),
                Message::user("Thanks."),
            ]
        );
    }

    #[test]
    fn an_image_is_an_image() {
        let request = sampling_request(
            "qwen3",
            CreateMessageRequestParams::new().with_message(
                SamplingMessage::user()
                    .with("What is on this chart?")
                    .with(Content::image(vec![1, 2, 3])),
            ),
        )
        .expect("a request");

        assert_eq!(
            request.messages[0].parts,
            [
                Part::Text("What is on this chart?".into()),
                Part::Image(Image::bytes(vec![1, 2, 3], "image/jpg")),
            ]
        );
    }

    /// Dropping any of these would change what the model is asked without
    /// the server knowing.
    #[test]
    fn what_a_model_cannot_take_is_an_error() {
        let audio = CreateMessageRequestParams::new()
            .with_message(SamplingMessage::user().with(Content::audio(vec![1, 2, 3])));
        let stops = CreateMessageRequestParams::new()
            .with_message("Count to ten.")
            .with_stop_seq(vec!["5".into()]);

        for (params, what) in [(audio, "Audio"), (stops, "Stop sequences")] {
            let err = sampling_request("qwen3", params).expect_err("refused");
            assert!(err.to_string().contains(what), "{err}");
        }
    }

    #[test]
    fn a_required_call_is_asked_of_the_model() {
        let request = sampling_request(
            "qwen3",
            CreateMessageRequestParams::new()
                .with_message("2 + 3?")
                .with_tools([serde_json::from_value(add_tool()).expect("a tool")])
                .with_tool_choice(ToolChoiceMode::Required),
        )
        .expect("a request");

        assert_eq!(request.tools.len(), 1);
        assert_eq!(request.tool_choice, ToolChoice::Required);
    }

    /// A call required with nothing to call is one no model can make.
    #[test]
    fn a_required_call_needs_tools() {
        let params = CreateMessageRequestParams::new()
            .with_message("2 + 3?")
            .with_tool_choice(ToolChoiceMode::Required);
        let err = sampling_request("qwen3", params).expect_err("refused");
        assert!(err.to_string().contains("toolChoice: required"), "{err}");
    }

    #[test]
    fn no_tool_choice_leaves_the_tools_out() {
        let request = sampling_request(
            "qwen3",
            CreateMessageRequestParams::new()
                .with_message("2 + 3?")
                .with_tools([serde_json::from_value(add_tool()).expect("a tool")])
                .with_tool_choice(ToolChoiceMode::None),
        )
        .expect("a request");

        assert!(request.tools.is_empty());
    }

    #[test]
    fn misplaced_sampling_content_is_an_error() {
        for message in [
            json!({ "role": "user", "content": { "type": "tool_use", "id": "c", "name": "add", "input": {} } }),
            json!({
                "role": "assistant",
                "content": { "type": "tool_result", "toolUseId": "c", "content": [] }
            }),
        ] {
            let params = params(json!({ "messages": [message], "maxTokens": 10 }));
            assert!(sampling_request("qwen3", params).is_err());
        }
    }

    #[test]
    fn max_tokens_must_be_positive() {
        let params = CreateMessageRequestParams::new()
            .with_message("2 + 3?")
            .with_max_tokens(0);
        let err = sampling_request("qwen3", params).expect_err("refused");
        assert!(err.to_string().contains("maxTokens"), "{err}");
    }

    #[test]
    fn a_call_is_a_tool_use_that_keeps_its_id() {
        let request = Request::new("qwen3");
        let mut done = Completion::new(FinishReason::ToolCalls);
        done.text = "Adding.".into();
        done.calls = vec![ToolCall::new("call-1", "add", r#"{"a": 2, "b": 3}"#)];

        let sample = sampling_result(&request, done).expect("a sample");

        assert_eq!(sample.model, "qwen3");
        assert_eq!(sample.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(
            sample.text().next().map(|t| t.text.as_str()),
            Some("Adding.")
        );

        let call = sample.tools().next().expect("a tool use");
        assert_eq!(call.id, "call-1");
        assert_eq!(call.name, "add");
        let input = call.input.as_ref().expect("input");
        assert_eq!(input["a"], 2);
        assert_eq!(input["b"], 3);
    }

    #[test]
    fn the_stop_reason_follows_the_finish() {
        let request = Request::new("qwen3");
        for (finish, stop) in [
            (FinishReason::Stop, StopReason::EndTurn),
            (FinishReason::Length, StopReason::MaxTokens),
            (
                FinishReason::ContentFilter,
                StopReason::Other("contentFilter".into()),
            ),
            (FinishReason::Refusal, StopReason::Other("refusal".into())),
        ] {
            let sample = sampling_result(&request, Completion::new(finish)).expect("a sample");
            assert_eq!(sample.stop_reason, Some(stop));
        }
    }

    /// A sample has content even when the model said nothing.
    #[test]
    fn an_empty_answer_is_empty_text() {
        let sample = sampling_result(
            &Request::new("qwen3"),
            Completion::new(FinishReason::Length),
        )
        .expect("a sample");

        assert_eq!(
            sample.text().map(|t| t.text.as_str()).collect::<Vec<_>>(),
            [""]
        );
    }

    #[test]
    fn arguments_that_are_not_an_object_are_an_error() {
        let mut done = Completion::new(FinishReason::ToolCalls);
        done.calls = vec![ToolCall::new("call-1", "add", "[2, 3]")];

        let err = sampling_result(&Request::new("qwen3"), done).expect_err("refused");
        assert!(err.to_string().contains("`add`"), "{err}");
    }
}

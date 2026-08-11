//! Server fixture for the official MCP conformance suite.
//!
//! The suite connects to this binary as an MCP client and drives every server
//! scenario against it, so the primitives below are not a demo -- their names
//! and shapes are the contract the scenarios assert. See `conformance/README.md`
//! for how to run it.
//!
//! ```no_rust
//! PORT=3000 cargo run -p neva-conformance --bin conformance-server
//! npx @modelcontextprotocol/conformance server \
//!     --url http://127.0.0.1:3000/mcp --requirements 2026-07-28
//! ```

use neva::prelude::*;
use serde_json::json;
use tracing_subscriber::{filter, prelude::*};

/// 1x1 transparent PNG -- the smallest valid image the suite accepts. Content
/// carries raw bytes and base64-encodes them on the wire, so this is the
/// decoded form, not the base64 the scenario descriptions quote.
const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xfc, 0xcf, 0xc0, 0x50,
    0x0f, 0x00, 0x04, 0x85, 0x01, 0x80, 0x84, 0xa9, 0x8c, 0x21, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// Minimal 44-byte WAV header with no samples -- enough to carry a mime type.
const WAV_SILENCE: &[u8] = &[
    0x52, 0x49, 0x46, 0x46, 0x24, 0x00, 0x00, 0x00, 0x57, 0x41, 0x56, 0x45, 0x66, 0x6d, 0x74, 0x20,
    0x10, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x44, 0xac, 0x00, 0x00, 0x88, 0x58, 0x01, 0x00,
    0x02, 0x00, 0x10, 0x00, 0x64, 0x61, 0x74, 0x61, 0x00, 0x00, 0x00, 0x00,
];

/// The image fixture as tool content.
fn png_content() -> Content {
    ImageContent::new(PNG_1X1).with_mime("image/png").into()
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

#[tool(descr = "Returns simple text content")]
async fn test_simple_text() -> &'static str {
    "This is a simple text response for testing."
}

#[tool(descr = "Returns image content")]
async fn test_image_content() -> Result<Content, Error> {
    Ok(png_content())
}

#[tool(descr = "Returns audio content")]
async fn test_audio_content() -> Result<Content, Error> {
    Ok(AudioContent::new(WAV_SILENCE).with_mime("audio/wav").into())
}

#[tool(descr = "Returns an embedded resource")]
async fn test_embedded_resource() -> Result<Content, Error> {
    Ok(Content::resource(
        TextResourceContents::new("test://embedded-resource", "Embedded resource content")
            .with_mime("text/plain"),
    ))
}

#[tool(descr = "Returns multiple content types")]
async fn test_multiple_content_types() -> Result<Vec<Content>, Error> {
    Ok(vec![
        Content::text("Multiple content types test:"),
        png_content(),
        Content::resource(
            TextResourceContents::new(
                "test://mixed-content-resource",
                r#"{"test":"data","value":123}"#,
            )
            .with_mime("application/json"),
        ),
    ])
}

#[tool(descr = "Emits log messages while running")]
async fn test_tool_with_logging() -> &'static str {
    tracing::debug!("Debug message from test_tool_with_logging");
    tracing::info!("Info message from test_tool_with_logging");
    tracing::warn!("Warning message from test_tool_with_logging");
    tracing::error!("Error message from test_tool_with_logging");
    "Logging complete"
}

/// The scenario spells this tool out: report `0/100`, wait ~50ms, `50/100`,
/// wait ~50ms, `100/100`. The waits are not decoration. On the legacy transport
/// the reports travel on the session's SSE `GET` stream while the result travels
/// on the call's own `POST` response -- two connections, so nothing orders them
/// but the work between the reports. A tool that reports three times and returns
/// without ever awaiting never yields, and its result overtakes every report it
/// just made; the client, which stops looking once the result is in hand, sees
/// none of them.
#[tool(descr = "Reports progress while running")]
async fn test_tool_with_progress(token: Meta<ProgressToken>) -> &'static str {
    for value in [0, 50, 100] {
        tracing::info!(
            target: "progress",
            token = %token,
            value = value,
            total = 100,
            message = "working"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    "Progress complete"
}

#[tool(descr = "Returns a tool execution error")]
async fn test_error_handling() -> Result<String, Error> {
    Err(Error::new(
        ErrorCode::InternalError,
        "This tool always fails for testing purposes",
    ))
}

// ---------------------------------------------------------------------------
// Diagnostic tools (server-stateless)
//
// The stateless scenario probes structural rules through these three. They are
// listed so the scenario can tell "the server does not enforce this" from "the
// server gave me nothing to enforce it with".
// ---------------------------------------------------------------------------

/// Requires a capability the probe deliberately does not declare, so the server
/// must answer `-32021 MissingRequiredClientCapability`.
#[cfg(not(feature = "legacy-spec"))]
#[tool(descr = "Requires the sampling capability the caller may not have declared")]
async fn test_missing_capability(mut ctx: Context) -> Result<String, Error> {
    let params = CreateMessageRequestParams::new()
        .with_message(SamplingMessage::user().with("ping"))
        .with_max_tokens(8);
    #[allow(deprecated)]
    let res = ctx.sample("capability_probe", params).await?;
    Ok(format!("{:?}", res.content))
}

/// Mutates the tool list so an open `subscriptions/listen` stream must receive
/// `notifications/tools/list_changed`.
#[tool(descr = "Adds or removes a tool to trigger tools/list_changed")]
async fn test_trigger_tool_change(mut ctx: Context) -> Result<String, Error> {
    const NAME: &str = "test_dynamic_tool";
    if ctx.find_tool(NAME).await.is_some() {
        ctx.remove_tool(NAME).await?;
        Ok(format!("{NAME} removed"))
    } else {
        let mut tool = Tool::new(NAME, || async { "dynamically added tool" });
        tool.with_description("A tool registered while the server was running");
        ctx.add_tool(tool).await?;
        Ok(format!("{NAME} added"))
    }
}

/// The prompt-list counterpart of [`test_trigger_tool_change`].
#[tool(descr = "Adds or removes a prompt to trigger prompts/list_changed")]
async fn test_trigger_prompt_change(mut ctx: Context) -> Result<String, Error> {
    const NAME: &str = "test_dynamic_prompt";
    if ctx.remove_prompt(NAME).await?.is_some() {
        return Ok(format!("{NAME} removed"));
    }
    let mut prompt = Prompt::new(NAME, || async {
        PromptMessage::user().with("dynamically added prompt")
    });
    prompt.with_description("A prompt registered while the server was running");
    ctx.add_prompt(prompt).await?;
    Ok(format!("{NAME} added"))
}

/// Emits log records; the scenario calls it *without* `_meta.../logLevel` and
/// asserts that nothing is pushed back.
#[tool(descr = "Emits log records at every level")]
async fn test_logging_tool() -> &'static str {
    tracing::debug!("debug from test_logging_tool");
    tracing::info!("info from test_logging_tool");
    tracing::warn!("warning from test_logging_tool");
    "Logged"
}

/// Produces a response *stream*: the scenario reads its frames and asserts the
/// server never puts an independent JSON-RPC request on one.
#[cfg(not(feature = "legacy-spec"))]
#[tool(descr = "Elicits input so the response is a stream")]
async fn test_streaming_elicitation(mut ctx: Context) -> Result<String, Error> {
    let params = ElicitRequestParams::form("Streaming elicitation")
        .with_required("name", "string")
        .into();
    let res = ctx.elicit("stream_input", params).await?;
    Ok(format!("{:?}", res.content))
}

/// Publishes a JSON Schema 2020-12 `inputSchema` using the whole vocabulary the
/// suite checks for preservation: `$defs`, `$anchor`, `$ref`, `allOf`/`anyOf`
/// and the `if`/`then`/`else` triple.
#[tool(
    descr = "Publishes a JSON Schema 2020-12 input schema",
    input_schema = r##"{
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "$defs": {
            "address": {
                "$anchor": "addressDef",
                "type": "object",
                "properties": {
                    "street": { "type": "string" },
                    "city": { "type": "string" }
                }
            }
        },
        "properties": {
            "name": { "type": "string" },
            "address": { "$ref": "#/$defs/address" },
            "contactMethod": { "type": "string", "enum": ["phone", "email"] },
            "phone": { "type": "string" },
            "email": { "type": "string" }
        },
        "allOf": [{ "anyOf": [{ "required": ["phone"] }, { "required": ["email"] }] }],
        "if": {
            "properties": { "contactMethod": { "const": "phone" } },
            "required": ["contactMethod"]
        },
        "then": { "required": ["phone"] },
        "else": { "required": ["email"] },
        "additionalProperties": false
    }"##
)]
async fn json_schema_2020_12_tool(name: Option<String>) -> String {
    format!("Received {}", name.unwrap_or_else(|| "(nothing)".into()))
}

// ---------------------------------------------------------------------------
// Legacy-profile fixtures
//
// Before 2026-07-28 the server *pushed* sampling and elicitation requests at
// the client instead of returning them as MRTR input requests, so these two
// exist only on that profile. Their 2026-07-28 counterparts are the
// `test_input_required_result_*` tools below.
// ---------------------------------------------------------------------------

#[cfg(feature = "legacy-spec")]
#[tool(descr = "Requests an LLM completion from the client")]
async fn test_sampling(mut ctx: Context) -> Result<String, Error> {
    let params = CreateMessageRequestParams::new()
        .with_message(SamplingMessage::user().with("What is the capital of France?"))
        .with_max_tokens(100);
    let res = ctx.sample(params).await?;
    Ok(format!("The model said: {:?}", res.content))
}

#[cfg(feature = "legacy-spec")]
#[tool(descr = "Requests user input from the client")]
async fn test_elicitation(mut ctx: Context) -> Result<String, Error> {
    let params = ElicitRequestParams::form("What is your name?")
        .with_required("name", "string")
        .into();
    let res = ctx.elicit(params).await?;
    Ok(format!("{:?}", res.content))
}

/// SEP-1034: every primitive type carries a `default`.
#[cfg(feature = "legacy-spec")]
#[tool(descr = "Elicits a form whose fields all carry defaults")]
async fn test_elicitation_sep1034_defaults(mut ctx: Context) -> Result<String, Error> {
    let params = ElicitRequestParams::form("Please confirm your details")
        .with_required("name", &json!({ "type": "string", "default": "John Doe" }))
        .with_required("age", &json!({ "type": "integer", "default": 30 }))
        .with_required("score", &json!({ "type": "number", "default": 95.5 }))
        .with_required(
            "status",
            &json!({
                "type": "string",
                "enum": ["active", "inactive", "pending"],
                "default": "active"
            }),
        )
        .with_required("verified", &json!({ "type": "boolean", "default": true }))
        .into();
    let res = ctx.elicit(params).await?;
    Ok(format!(
        "Elicitation completed: action={:?}, content={:?}",
        res.action, res.content
    ))
}

/// SEP-1330: all five ways the spec lets an enum be spelled.
#[cfg(feature = "legacy-spec")]
#[tool(descr = "Elicits a form exercising every enum variant")]
async fn test_elicitation_sep1330_enums(mut ctx: Context) -> Result<String, Error> {
    let params = ElicitRequestParams::form("Pick your options")
        .with_required(
            "untitledSingle",
            &json!({ "type": "string", "enum": ["option1", "option2", "option3"] }),
        )
        .with_required(
            "titledSingle",
            &json!({
                "type": "string",
                "oneOf": [
                    { "const": "value1", "title": "First Option" },
                    { "const": "value2", "title": "Second Option" },
                    { "const": "value3", "title": "Third Option" }
                ]
            }),
        )
        .with_required(
            "legacyEnum",
            &json!({
                "type": "string",
                "enum": ["opt1", "opt2", "opt3"],
                "enumNames": ["Option One", "Option Two", "Option Three"]
            }),
        )
        .with_required(
            "untitledMulti",
            &json!({
                "type": "array",
                "items": { "type": "string", "enum": ["option1", "option2", "option3"] }
            }),
        )
        .with_required(
            "titledMulti",
            &json!({
                "type": "array",
                "items": {
                    "anyOf": [
                        { "const": "value1", "title": "First Choice" },
                        { "const": "value2", "title": "Second Choice" },
                        { "const": "value3", "title": "Third Choice" }
                    ]
                }
            }),
        )
        .into();
    let res = ctx.elicit(params).await?;
    Ok(format!(
        "Elicitation completed: action={:?}, content={:?}",
        res.action, res.content
    ))
}

// ---------------------------------------------------------------------------
// MRTR fixtures (SEP-2322)
//
// One tool per input-request kind, plus the multi-round and state variants. The
// input keys are part of the contract -- the scenarios look them up by name.
// ---------------------------------------------------------------------------

#[cfg(not(feature = "legacy-spec"))]
#[tool(descr = "Elicits a name, then greets it")]
async fn test_input_required_result_elicitation(mut ctx: Context) -> Result<String, Error> {
    let params = ElicitRequestParams::form("What is your name?")
        .with_required("name", "string")
        .into();
    let name = elicited_name(ctx.elicit("user_name", params).await?);
    Ok(format!("Hello, {name}!"))
}

#[cfg(not(feature = "legacy-spec"))]
#[tool(descr = "Asks the client's model a question")]
async fn test_input_required_result_sampling(mut ctx: Context) -> Result<String, Error> {
    let params = CreateMessageRequestParams::new()
        .with_message(SamplingMessage::user().with("What is the capital of France?"))
        .with_max_tokens(100);
    #[allow(deprecated)]
    let res = ctx.sample("capital_question", params).await?;
    Ok(format!("The model said: {:?}", res.content))
}

#[cfg(not(feature = "legacy-spec"))]
#[tool(descr = "Asks the client for its roots")]
async fn test_input_required_result_list_roots(mut ctx: Context) -> Result<String, Error> {
    #[allow(deprecated)]
    let roots = ctx.list_roots("client_roots").await?;
    Ok(format!("Client exposes {} root(s)", roots.roots.len()))
}

#[cfg(not(feature = "legacy-spec"))]
#[tool(descr = "Carries server state across the round-trip")]
async fn test_input_required_result_request_state(mut ctx: Context) -> Result<String, Error> {
    // `memo` is what puts server-computed data into `requestState`: it is
    // sealed into the blob on round 1 and replayed on round 2.
    let ticket: String = ctx
        .memo("ticket", async { Ok("ticket-42".to_string()) })
        .await?;
    let params = ElicitRequestParams::form("Confirm the ticket")
        .with_required("name", "string")
        .into();
    let name = elicited_name(ctx.elicit("confirmation", params).await?);
    Ok(format!("{name} confirmed {ticket}"))
}

/// Three inputs, one round.
///
/// The `?`s are deliberately held until every input has been asked for. Each
/// helper records its request and returns the same "input required" signal, so
/// unwinding at the first one would put a single request in the round and cost
/// three round-trips for what fits in one.
#[cfg(not(feature = "legacy-spec"))]
#[tool(descr = "Asks for several inputs at once")]
async fn test_input_required_result_multiple_inputs(mut ctx: Context) -> Result<String, Error> {
    let form = ElicitRequestParams::form("What is your name?")
        .with_required("name", "string")
        .into();
    let sampling = CreateMessageRequestParams::new()
        .with_message(SamplingMessage::user().with("Generate a greeting"))
        .with_max_tokens(50);

    let name = ctx.elicit("user_name", form).await;
    #[allow(deprecated)]
    let greeting = ctx.sample("greeting", sampling).await;
    #[allow(deprecated)]
    let roots = ctx.list_roots("client_roots").await;

    let (name, greeting, roots) = (elicited_name(name?), greeting?, roots?);
    Ok(format!(
        "{name}: {:?} ({} roots)",
        greeting.content,
        roots.roots.len()
    ))
}

#[cfg(not(feature = "legacy-spec"))]
#[tool(descr = "Needs two separate rounds of input")]
async fn test_input_required_result_multi_round(mut ctx: Context) -> Result<String, Error> {
    let first = ElicitRequestParams::form("Step 1: your name?")
        .with_required("name", "string")
        .into();
    let step1 = elicited_name(ctx.elicit("step1", first).await?);

    let second = ElicitRequestParams::form("Step 2: confirm?")
        .with_required("name", "string")
        .into();
    let step2 = elicited_name(ctx.elicit("step2", second).await?);

    Ok(format!("Completed both rounds: {step1} / {step2}"))
}

#[cfg(not(feature = "legacy-spec"))]
#[tool(descr = "Elicits input so a tampered requestState can be replayed at it")]
async fn test_input_required_result_tampered_state(mut ctx: Context) -> Result<String, Error> {
    let params = ElicitRequestParams::form("State check")
        .with_required("name", "string")
        .into();
    let name = elicited_name(ctx.elicit("state_check", params).await?);
    Ok(format!("State accepted for {name}"))
}

/// Asks for whatever the caller said it can answer, and nothing else.
///
/// Asking for an undeclared kind is refused outright, which ends the call --
/// so a tool that can get its answer more than one way has to look at the
/// declaration before it asks, not after it is refused.
#[cfg(not(feature = "legacy-spec"))]
#[tool(descr = "Asks only for the input kinds the caller declared")]
async fn test_input_required_result_capabilities(mut ctx: Context) -> Result<String, Error> {
    let declared = ctx.client_capabilities();

    // Down to the mode: a caller that named only `url` has not said it can fill
    // in a form, and asking it to anyway is the refusal this tool exists to
    // avoid.
    let params: ElicitRequestParams = ElicitRequestParams::form("Capability check")
        .with_required("name", "string")
        .into();
    if declared
        .elicitation
        .is_some_and(|modes| modes.allows(&params))
    {
        let name = elicited_name(ctx.elicit("capability_check", params).await?);
        return Ok(format!("Capability satisfied for {name}"));
    }

    if declared.sampling {
        let params = CreateMessageRequestParams::new()
            .with_message(SamplingMessage::user().with("What is your name?"))
            .with_max_tokens(50);
        #[allow(deprecated)]
        let answer = ctx.sample("capability_check", params).await?;
        return Ok(format!("Capability satisfied by {:?}", answer.content));
    }

    Ok("Caller can answer nothing; nothing was asked".to_string())
}

#[cfg(not(feature = "legacy-spec"))]
#[prompt(descr = "A prompt that needs elicited input before it can render")]
async fn test_input_required_result_prompt(mut ctx: Context) -> Result<PromptMessage, Error> {
    let params = ElicitRequestParams::form("What is your name?")
        .with_required("name", "string")
        .into();
    let name = elicited_name(ctx.elicit("user_name", params).await?);
    Ok(PromptMessage::user().with(format!("Hello, {name}!")))
}

/// The `name` field out of an elicitation result, or a stand-in when the client
/// declined -- a fixture must still produce a complete result.
#[cfg(not(feature = "legacy-spec"))]
fn elicited_name(res: ElicitResult) -> String {
    res.content
        .and_then(|c| c.get("name").and_then(|v| v.as_str().map(str::to_owned)))
        .unwrap_or_else(|| "anonymous".into())
}

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

#[resources]
async fn list_resources(_params: ListResourcesRequestParams) -> ListResourcesResult {
    [
        Resource::new("test://static-text", "Static Text Resource")
            .with_descr("A static text resource for testing")
            .with_mime("text/plain"),
        Resource::new("test://static-binary", "Static Binary Resource")
            .with_descr("A static binary resource for testing")
            .with_mime("image/png"),
        Resource::new("test://watched-resource", "Watched Resource")
            .with_descr("A resource that emits update notifications")
            .with_mime("text/plain"),
    ]
    .into()
}

#[resource(uri = "test://static-text", mime = "text/plain")]
async fn static_text(uri: Uri) -> TextResourceContents {
    TextResourceContents::new(
        uri,
        "This is a static text resource for conformance testing.",
    )
    .with_mime("text/plain")
}

#[resource(uri = "test://static-binary", mime = "image/png")]
async fn static_binary(uri: Uri) -> BlobResourceContents {
    BlobResourceContents::new(uri, PNG_1X1).with_mime("image/png")
}

#[resource(uri = "test://watched-resource", mime = "text/plain")]
async fn watched_resource(uri: Uri) -> TextResourceContents {
    TextResourceContents::new(uri, "Watched resource content").with_mime("text/plain")
}

#[resource(uri = "test://template/{id}/data", mime = "application/json")]
async fn template_resource(uri: Uri, id: String) -> TextResourceContents {
    let body = json!({ "id": id, "data": "template data" });
    TextResourceContents::new(uri, body.to_string()).with_mime("application/json")
}

// ---------------------------------------------------------------------------
// Prompts
// ---------------------------------------------------------------------------

#[prompt(descr = "A simple prompt without arguments")]
async fn test_simple_prompt() -> PromptMessage {
    PromptMessage::user().with("This is a simple prompt for testing.")
}

#[prompt(
    descr = "A prompt with arguments",
    args = r#"[
        { "name": "arg1", "description": "First argument", "required": true },
        { "name": "arg2", "description": "Second argument", "required": false }
    ]"#
)]
async fn test_prompt_with_arguments(arg1: String, arg2: Option<String>) -> PromptMessage {
    PromptMessage::user().with(format!(
        "Prompt with arguments: arg1={arg1}, arg2={}",
        arg2.unwrap_or_else(|| "(none)".into())
    ))
}

#[prompt(
    descr = "A prompt with an embedded resource",
    args = r#"[
        { "name": "resource_uri", "description": "URI of the resource to embed", "required": false }
    ]"#
)]
async fn test_prompt_with_embedded_resource(resource_uri: Option<String>) -> PromptMessage {
    let uri = resource_uri.unwrap_or_else(|| "test://example-resource".into());
    PromptMessage::user().with(Content::resource(
        TextResourceContents::new(uri, "Embedded resource content in prompt")
            .with_mime("text/plain"),
    ))
}

#[prompt(descr = "A prompt with image content")]
async fn test_prompt_with_image() -> PromptMessage {
    PromptMessage::user().with(png_content())
}

// ---------------------------------------------------------------------------
// Completion
// ---------------------------------------------------------------------------

#[completion]
async fn complete(params: CompleteRequestParams) -> Completion {
    let candidates = ["conformance", "completion", "coverage"];
    let prefix = params.arg.value.to_lowercase();
    candidates
        .iter()
        .filter(|c| c.starts_with(&prefix))
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .into()
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(filter::LevelFilter::TRACE)
        .with(notification::fmt::layer())
        .init();

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    let addr = format!("127.0.0.1:{port}");

    App::new()
        .with_options(|opt| {
            opt.with_name("neva conformance server")
                .with_version(env!("CARGO_PKG_VERSION"))
                .with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
                .with_tools(|tools| tools.with_list_changed())
                .with_prompts(|prompts| prompts.with_list_changed())
                .with_resources(|res| res.with_list_changed().with_subscribe())
        })
        .run()
        .await;
}

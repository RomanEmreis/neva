//! Sampling through svir: a server samples twice with a tool, and the client
//! answers through `sampling_request` and `sampling_result` with a model
//! played by hand. What is checked is what the model is asked and what the
//! server gets back, in both protocol profiles: server-pushed under
//! `legacy-spec`, MRTR input requests under 2026-07-28.
#![cfg(all(
    feature = "svir",
    feature = "http-server-volga",
    feature = "http-client"
))]

use neva::{
    App, Context,
    client::Client,
    error::Error,
    types::{
        CallToolResponse, Tool, ToolResult,
        sampling::{CreateMessageRequestParams, CreateMessageResult, SamplingMessage},
    },
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use svir::{Completion, FinishReason, Role, ToolCall};

/// A model that adds with the tool it is given: it calls `add` until it has
/// the tool's result, then answers with it.
fn adder(request: &svir::Request) -> Result<Completion, svir::Error> {
    let told = request
        .messages
        .iter()
        .filter(|message| message.role == Role::Tool)
        .flat_map(|message| &message.parts)
        .find_map(|part| match part {
            svir::Part::ToolResult(result) => Some(result.content.clone()),
            _ => None,
        });

    Ok(match told {
        Some(sum) => {
            let mut done = Completion::new(FinishReason::Stop);
            done.text = format!("2 + 3 = {sum}");
            done
        }
        None => {
            let mut done = Completion::new(FinishReason::ToolCalls);
            done.calls = vec![ToolCall::new("call-1", "add", r#"{"a": 2, "b": 3}"#)];
            done
        }
    })
}

/// A model server that refuses the request, and says more than it should.
fn overflowing(_request: &svir::Request) -> Result<Completion, svir::Error> {
    Err(svir::Error::new(svir::ErrorKind::ContextOverflow)
        .with_server_message("This model's maximum context length is 8192 tokens (org-secret)"))
}

#[allow(deprecated)]
async fn sample(
    ctx: &Context,
    key: &str,
    params: CreateMessageRequestParams,
) -> Result<CreateMessageResult, Error> {
    #[cfg(not(feature = "legacy-spec"))]
    let sampled = ctx.sample(key, params).await;
    #[cfg(feature = "legacy-spec")]
    let sampled = {
        let _ = key;
        ctx.sample(params).await
    };
    sampled
}

fn add_tool() -> Tool {
    serde_json::from_value(serde_json::json!({
        "name": "add",
        "description": "Adds two numbers.",
        "inputSchema": {
            "type": "object",
            "properties": { "a": { "type": "integer" }, "b": { "type": "integer" } }
        }
    }))
    .expect("a tool")
}

/// Serves `add_up`, which asks the client's model what 2 + 3 is, runs the
/// `add` call the model makes, and samples again with its result; and
/// `ask`, which samples once and tells what went wrong, if anything did.
async fn serve(
    model: fn(&svir::Request) -> Result<Completion, svir::Error>,
) -> (
    Client,
    Arc<Mutex<Vec<svir::Request>>>,
    tokio::task::JoinHandle<()>,
) {
    let addr = format!("127.0.0.1:{}", pick_free_port());

    let app = App::new().without_greeting();
    #[cfg(not(feature = "legacy-spec"))]
    let app = app.with_request_state_secret(b"test-secret");
    let mut app = app.with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")));

    app.map_tool("add_up", |ctx: Context| async move {
        let ask = CreateMessageRequestParams::new()
            .with_sys_prompt("You add numbers with the tools you have.")
            .with_message("What is 2 + 3?")
            .with_max_tokens(100)
            .with_tools([add_tool()]);

        let first = sample(&ctx, "first", ask.clone()).await?;
        let call = first
            .tools()
            .next()
            .cloned()
            .expect("the model calls `add`");
        let input = call.input.clone().unwrap_or_default();
        let sum = input["a"].as_i64().unwrap_or_default() + input["b"].as_i64().unwrap_or_default();

        let result = ToolResult::new(call.id.clone(), CallToolResponse::new(sum.to_string()));
        let follow = ask
            .with_message(SamplingMessage::assistant().with(call))
            .with_message(SamplingMessage::user().with(result));
        let second = sample(&ctx, "second", follow).await?;

        Ok::<String, Error>(second.text().map(|text| text.text.clone()).collect())
    });
    app.map_tool("ask", |ctx: Context| async move {
        let ask = CreateMessageRequestParams::new().with_message("Summarise the week.");
        Ok::<String, Error>(match sample(&ctx, "ask", ask).await {
            Ok(_) => "answered".into(),
            Err(err) => format!("failed: {err}"),
        })
    });

    let server = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let seen: Arc<Mutex<Vec<svir::Request>>> = Arc::default();
    let log = seen.clone();
    let mut client = Client::new().with_options(|o| {
        o.with_http(|h| h.bind(&addr).with_endpoint("/mcp"))
            .with_timeout(Duration::from_secs(10))
    });
    #[allow(deprecated)]
    client.map_sampling(move |params: CreateMessageRequestParams| {
        let log = log.clone();
        async move {
            let request = neva::svir::sampling_request("adder-1", params)?;
            log.lock().expect("the log").push(request.clone());
            let done = model(&request)?;
            neva::svir::sampling_result(&request, done)
        }
    });
    client.connect().await.expect("connect");

    (client, seen, server)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_samples_a_model_that_calls_a_tool() {
    let (client, seen, server) = serve(adder).await;

    let answer = client
        .tools()
        .call("add_up", ())
        .await
        .expect("the call completes");
    let text = answer
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.as_str());
    assert_eq!(text, Some("2 + 3 = 5"));

    let seen = std::mem::take(&mut *seen.lock().expect("the log"));
    assert_eq!(seen.len(), 2, "the model is asked twice");

    // The first round: the server's prompt, its tool, its limit.
    let first = &seen[0];
    assert_eq!(first.model, "adder-1");
    assert_eq!(
        first.system.as_deref(),
        Some("You add numbers with the tools you have.")
    );
    assert_eq!(first.max_tokens, Some(100));
    assert_eq!(
        first
            .tools
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        ["add"]
    );
    assert_eq!(first.messages, [svir::Message::user("What is 2 + 3?")]);

    // The second: the model's own call, found again by its id, and its result.
    assert_eq!(
        seen[1].messages,
        [
            svir::Message::user("What is 2 + 3?"),
            svir::Message::new(Role::Assistant).with(ToolCall::new(
                "call-1",
                "add",
                r#"{"a":2,"b":3}"#
            )),
            svir::Message::tool_result(svir::ToolResult::new("call-1", "5")),
        ]
    );

    client.disconnect().await.ok();
    server.abort();
}

/// Under `legacy-spec` the server is answered with the failure; under
/// 2026-07-28 there is nothing to answer it with, and the call fails. Either
/// way the model server's own message stays on the client.
#[tokio::test(flavor = "multi_thread")]
async fn a_model_that_fails_is_a_failed_sample() {
    let (client, _seen, server) = serve(overflowing).await;

    let outcome = client.tools().call("ask", ()).await;

    #[cfg(feature = "legacy-spec")]
    let told = {
        let answer = outcome.expect("the tool tells the failure");
        let text = answer
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone());
        text.expect("text")
    };
    #[cfg(not(feature = "legacy-spec"))]
    let told = outcome.expect_err("the call fails").to_string();

    assert!(
        told.contains("does not fit in the model's context"),
        "{told}"
    );
    assert!(!told.contains("org-secret"), "{told}");

    client.disconnect().await.ok();
    server.abort();
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

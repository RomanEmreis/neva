//! `LocalTools`: a server's own tools, handed to a model in the same process
//! through the pipeline a call over MCP takes.
//!
//! The model is played by hand-built `ToolCall`s. The `#[tool]` functions
//! below are registered into every `App` of this test binary.
#![cfg(all(
    feature = "svir",
    feature = "server-macros",
    feature = "di",
    feature = "http-server"
))]

use neva::{
    App, Context,
    di::Dc,
    error::{Error, ErrorCode},
    tool,
    types::{Response, elicitation::ElicitRequestParams},
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use svir::{ToolCall, Toolbox};

struct Greeting(&'static str);

#[tool(descr = "Adds two numbers.")]
async fn add(a: i64, b: i64) -> i64 {
    a + b
}

#[tool]
async fn greet(greeting: Dc<Greeting>, name: String) -> String {
    format!("{}, {name}!", greeting.0)
}

#[tool]
async fn count_tools(ctx: Context) -> String {
    ctx.tools().list().await.len().to_string()
}

#[tool]
async fn ask_name(ctx: Context) -> Result<String, Error> {
    let params: ElicitRequestParams = ElicitRequestParams::form("Your name?")
        .with_required("name", "string")
        .into();
    #[cfg(not(feature = "legacy-spec"))]
    let answer = ctx.elicit("name", params).await?;
    #[cfg(feature = "legacy-spec")]
    let answer = ctx.elicit(params).await?;
    Ok(format!("{:?}", answer.content))
}

#[tool(roles = ["admin"])]
async fn wipe() -> String {
    "wiped".to_string()
}

fn app() -> App {
    App::new()
        .without_greeting()
        .add_singleton(Greeting("Hello"))
}

fn call(id: &str, name: &str, arguments: &str) -> ToolCall {
    ToolCall::new(id, name, arguments)
}

#[tokio::test]
async fn a_tool_written_once_is_offered_to_a_model() {
    let tools = app().into_toolbox().load().await.expect("load");

    let mut names: Vec<_> = tools.tools().into_iter().map(|tool| tool.name).collect();
    names.sort();
    // `wipe` requires a role, and an in-process call has no claims.
    assert_eq!(names, ["add", "ask_name", "count_tools", "greet"]);

    let add = tools
        .tools()
        .into_iter()
        .find(|tool| tool.name == "add")
        .expect("add");
    assert_eq!(add.description, "Adds two numbers.");
    assert!(add.input_schema["properties"].get("a").is_some(), "{add:?}");

    let told = tools.call(&call("c1", "add", r#"{"a": 2, "b": 3}"#)).await;
    assert_eq!(told.call_id, "c1");
    assert_eq!(told.content, "5", "{told:?}");
    assert!(!told.is_error);
}

/// A tool gets its injected dependencies and its `Context` as over MCP.
#[tokio::test]
async fn a_tool_gets_its_dependencies_and_context() {
    let tools = app().into_toolbox().load().await.expect("load");

    let told = tools.call(&call("c1", "greet", r#"{"name": "Ann"}"#)).await;
    assert_eq!(told.content, "Hello, Ann!", "{told:?}");

    let told = tools.call(&call("c2", "count_tools", "")).await;
    assert_eq!(told.content, "5", "{told:?}");
}

/// There is no peer to ask: the call is refused at once, not left to wait
/// out the request timeout.
#[tokio::test]
async fn a_tool_that_asks_for_input_is_refused_at_once() {
    let tools = app().into_toolbox().load().await.expect("load");

    let told = tokio::time::timeout(
        Duration::from_secs(2),
        tools.call(&call("c1", "ask_name", "")),
    )
    .await
    .expect("answered at once");
    assert!(told.is_error, "{told:?}");
    // Under 2026-07-28 the caller declared no elicitation, and the request is
    // refused before it is made; under legacy-spec there is no session for it.
    assert!(told.content.contains("elicitation"), "{told:?}");
}

/// The server's middleware sees the model's calls as any other caller's.
#[tokio::test]
async fn the_middleware_sees_every_call() {
    let seen = Arc::new(AtomicUsize::new(0));
    let counted = seen.clone();
    let tools = app()
        .wrap_tools(move |ctx, next| {
            let counted = counted.clone();
            async move {
                counted.fetch_add(1, Ordering::SeqCst);
                next(ctx).await
            }
        })
        .into_toolbox()
        .load()
        .await
        .expect("load");

    let calls: Vec<_> = (0..4)
        .map(|i| call(&format!("c{i}"), "add", &format!(r#"{{"a": {i}, "b": 1}}"#)))
        .collect();
    let told = tools.call_all(&calls).await;

    let sums: Vec<_> = told.iter().map(|result| result.content.as_str()).collect();
    assert_eq!(sums, ["1", "2", "3", "4"]);
    assert_eq!(seen.load(Ordering::SeqCst), 4);
}

/// A middleware that answers without calling `next` is heard too.
#[tokio::test]
async fn a_middleware_can_refuse_a_call() {
    let tools = app()
        .wrap_tools(|ctx, _next| async move {
            Response::error(
                ctx.id(),
                Error::new(ErrorCode::InvalidRequest, "denied by policy"),
            )
        })
        .into_toolbox()
        .load()
        .await
        .expect("load");

    let told = tools.call(&call("c1", "add", r#"{"a": 1, "b": 1}"#)).await;
    assert!(told.is_error, "{told:?}");
    assert!(told.content.contains("denied by policy"), "{told:?}");
}

/// Only what the snapshot offers can be called: not a filtered-out tool, not
/// one that needs claims, not one that does not exist.
#[tokio::test]
async fn only_offered_tools_can_be_called() {
    let tools = app()
        .into_toolbox()
        .filter(|tool| tool.name != "greet")
        .prefixed("local_")
        .load()
        .await
        .expect("load");

    for name in ["greet", "local_greet", "wipe", "local_wipe", "nope"] {
        let told = tools.call(&call("c1", name, "{}")).await;
        assert!(told.is_error, "{name} -> {told:?}");
        assert!(told.content.contains("no tool named"), "{name} -> {told:?}");
    }

    let told = tools
        .call(&call("c2", "local_add", r#"{"a": 1, "b": 2}"#))
        .await;
    assert_eq!(told.content, "3", "{told:?}");
}

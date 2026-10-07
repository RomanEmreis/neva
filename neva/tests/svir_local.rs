//! `LocalTools`: a server's own tools, handed to a model in the same process
//! through the pipeline a call over MCP takes.
//!
//! The model is played by hand-built `ToolCall`s. The `#[tool]` functions
//! below are registered into every `App` of this test binary.
#![cfg(all(
    feature = "svir",
    feature = "server-macros",
    feature = "di",
    feature = "http-server-volga"
))]

use neva::{
    App, Context,
    di::Dc,
    error::{Error, ErrorCode},
    tool,
    types::{Response, Tool, elicitation::ElicitRequestParams},
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

/// Drives a "model" over the server's other tools.
#[tool]
async fn agent(ctx: Context) -> Result<String, Error> {
    let tools = ctx
        .tools()
        .toolbox()
        .filter(|tool| tool.name == "add")
        .load()
        .await?;
    let told = tools
        .call(&ToolCall::new("inner", "add", r#"{"a": 20, "b": 22}"#))
        .await;
    Ok(told.content)
}

/// What a toolbox taken in this handler offers: the tool itself is left out,
/// whatever it is renamed to.
#[tool]
async fn peers(ctx: Context) -> Result<String, Error> {
    let tools = ctx.tools().toolbox().with_prefix("x_").load().await?;
    Ok(names(&tools))
}

/// The same, for a tool that means to recurse.
#[tool]
async fn recurse(ctx: Context) -> Result<String, Error> {
    let tools = ctx.tools().toolbox().with_caller().load().await?;
    Ok(names(&tools))
}

/// Calls itself through its own toolbox until a call is refused. Each level
/// that got its call through adds an `x` to what the refusal said.
#[tool]
async fn dive(ctx: Context) -> Result<String, Error> {
    let tools = ctx.tools().toolbox().with_caller();
    descend(tools, "dive").await
}

/// The same, under a bound of two.
#[tool]
async fn dive_shallow(ctx: Context) -> Result<String, Error> {
    let tools = ctx.tools().toolbox().with_caller().with_max_depth(2);
    descend(tools, "dive_shallow").await
}

async fn descend(tools: neva::svir::LocalTools, name: &str) -> Result<String, Error> {
    let tools = tools
        .filter(|tool| tool.name.starts_with("dive"))
        .load()
        .await?;
    let told = tools.call(&ToolCall::new("down", name, "")).await;
    Ok(format!("x{}", told.content))
}

fn names(tools: &impl Toolbox) -> String {
    let mut names: Vec<_> = tools.tools().into_iter().map(|tool| tool.name).collect();
    names.sort();
    names.join(",")
}

#[tool]
async fn install(ctx: Context) -> Result<String, Error> {
    ctx.tools()
        .add(Tool::new("late", || async { "late" }))
        .await?;
    Ok("installed".to_string())
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
    assert_eq!(
        names,
        [
            "add",
            "agent",
            "ask_name",
            "count_tools",
            "dive",
            "dive_shallow",
            "greet",
            "install",
            "peers",
            "recurse"
        ]
    );

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
    assert_eq!(told.content, "11", "{told:?}");
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
        .with_prefix("local_")
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

/// A tool can hand the server's other tools to a model of its own.
#[tokio::test]
async fn a_tool_can_drive_a_model_over_the_others() {
    let tools = app().into_toolbox().load().await.expect("load");

    let told = tools.call(&call("c1", "agent", "")).await;
    assert_eq!(told.content, "42", "{told:?}");
}

/// A toolbox taken in a handler leaves out the tool that handler serves,
/// compared by the server's name whatever the renames, unless that tool asks
/// to be kept. The nested calls here go through the pipeline, so each one's
/// `Context` names its own tool.
#[tokio::test]
async fn a_handlers_toolbox_leaves_its_own_tool_out() {
    let tools = app().into_toolbox().load().await.expect("load");

    let peers = tools.call(&call("c1", "peers", "")).await.content;
    let offered: Vec<_> = peers.split(',').collect();
    assert!(!offered.contains(&"x_peers"), "{peers}");
    assert!(offered.contains(&"x_recurse"), "{peers}");

    let recurse = tools.call(&call("c2", "recurse", "")).await.content;
    let offered: Vec<_> = recurse.split(',').collect();
    assert!(offered.contains(&"recurse"), "{recurse}");
    assert!(offered.contains(&"peers"), "{recurse}");
}

/// A tool that keeps calling itself is stopped four in-process calls deep:
/// the fourth gets its call refused, with the reason, and each level above it
/// answers. A toolbox can set its own bound.
#[tokio::test]
async fn nested_calls_stop_at_the_depth_bound() {
    let tools = app().into_toolbox().load().await.expect("load");

    let told = tools.call(&call("c1", "dive", "")).await.content;
    assert!(
        told.starts_with("xxxx`dive` was not called"),
        "four levels, then the refusal: {told}"
    );
    assert!(told.contains("at most 4 deep"), "{told}");

    let told = tools.call(&call("c2", "dive_shallow", "")).await.content;
    assert!(
        told.starts_with("xx`dive_shallow` was not called"),
        "two levels under a bound of two: {told}"
    );
}

/// The snapshot is renewed when asked: a tool added at runtime is offered
/// after a refresh, and not before.
#[tokio::test]
async fn a_refresh_sees_tools_added_at_runtime() {
    let tools = app().into_toolbox().load().await.expect("load");

    let told = tools.call(&call("c1", "install", "")).await;
    assert_eq!(told.content, "installed", "{told:?}");
    assert!(tools.call(&call("c2", "late", "")).await.is_error);

    tools.refresh().await.expect("refresh");
    let told = tools.call(&call("c3", "late", "")).await;
    assert_eq!(told.content, "late", "{told:?}");
}

/// A server can serve over MCP and be a toolbox at once; the toolbox is bound
/// when the server runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_served_server_is_a_toolbox_too() {
    let addr = format!("127.0.0.1:{}", pick_free_port());
    let (app, tools) = app()
        .with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")))
        .with_toolbox();
    let server = tokio::spawn(app.run());

    let tools = tokio::time::timeout(Duration::from_secs(10), tools.load())
        .await
        .expect("bound once the server runs")
        .expect("load");
    let told = tools.call(&call("c1", "add", r#"{"a": 2, "b": 3}"#)).await;
    assert_eq!(told.content, "5", "{told:?}");

    server.abort();
}

/// Waiting for a server that is gone would never end.
#[tokio::test]
async fn a_toolbox_of_a_server_that_never_ran_says_so() {
    let (app, tools) = app().with_toolbox();
    drop(app);

    let err = tokio::time::timeout(Duration::from_secs(2), tools.load())
        .await
        .expect("answered at once")
        .expect_err("the server never ran");
    assert!(err.to_string().contains("stopped before it ran"), "{err}");
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

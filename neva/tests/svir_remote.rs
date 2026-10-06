//! `RemoteTools`: a server's tools, handed to a model as a svir `Toolbox`.
//!
//! The model is played by hand-built `ToolCall`s; what is checked is what it
//! would be offered and told.
#![cfg(all(
    feature = "svir",
    feature = "http-server-volga",
    feature = "http-client"
))]

use neva::{
    App,
    client::Client,
    error::{Error, ErrorCode},
    svir::RemoteTools,
    types::{CallToolResponse, Content, Role},
};
use std::{sync::Arc, time::Duration};
use svir::{Part, TextFile, ToolCall, Toolbox};

/// Serves `add`, `greet`, `fail`, `photo` and, with `apps`, an app-only
/// `cart`; plus `files.read` when `dotted` -- a name a model API cannot carry.
/// Also a `review` prompt and `notes://{day}` resources.
async fn serve(dotted: bool) -> (Arc<Client>, tokio::task::JoinHandle<()>) {
    let addr = format!("127.0.0.1:{}", pick_free_port());

    let mut app = App::new()
        .without_greeting()
        .with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")));
    app.map_tool("add", |a: i64, b: i64| async move { a + b })
        .with_description("Adds two numbers.")
        .with_arg_names(["a", "b"]);
    app.map_tool("greet", || async { "hello" });
    app.map_tool("fail", || async {
        Err::<String, _>(Error::new(ErrorCode::InternalError, "out of paper"))
    });
    app.map_tool("photo", || async {
        CallToolResponse::new(Content::image(vec![0x89, b'P', b'N', b'G']))
    });
    #[cfg(feature = "apps")]
    app.map_tool("cart", || async { "cart" })
        .with_ui("ui://shop/cart")
        .with_visibility([neva::types::UiVisibility::App]);
    if dotted {
        app.map_tool("files.read", || async { "contents" });
    }
    app.map_prompt("review", || async {
        ("Review this change.".to_string(), Role::User)
    });
    app.map_resource("notes://{day}", "notes", |day: String| async move {
        (
            format!("notes://{day}"),
            format!("Ship the bridge on {day}."),
        )
    });

    let server = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut client = Client::new().with_options(|o| {
        o.with_http(|h| h.bind(&addr).with_endpoint("/mcp"))
            .with_timeout(Duration::from_secs(10))
    });
    client.connect().await.expect("connect");

    (Arc::new(client), server)
}

fn call(id: &str, name: &str, arguments: &str) -> ToolCall {
    ToolCall::new(id, name, arguments)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_model_is_offered_the_tools_it_may_call() {
    let (client, server) = serve(false).await;

    let tools = RemoteTools::new(client)
        .prefixed("shop_")
        .filter(|tool| tool.name != "greet")
        // Filters add up, and see the server's names whatever the renames.
        .filter(|tool| tool.name != "fail")
        .load()
        .await
        .expect("load");

    let mut names: Vec<_> = tools.tools().into_iter().map(|tool| tool.name).collect();
    names.sort();
    // `cart` is for the app only; `greet` and `fail` are filtered out.
    assert_eq!(names, ["shop_add", "shop_photo"]);

    let add = tools
        .tools()
        .into_iter()
        .find(|tool| tool.name == "shop_add")
        .expect("add");
    assert_eq!(add.description, "Adds two numbers.");
    assert!(add.input_schema["properties"].get("a").is_some(), "{add:?}");

    server.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_call_is_forwarded_and_answered() {
    let (client, server) = serve(false).await;
    let tools = RemoteTools::new(client)
        .prefixed("shop_")
        .load()
        .await
        .expect("load");

    let told = tools
        .call(&call("c1", "shop_add", r#"{"a": 2, "b": 3}"#))
        .await;
    assert_eq!(told.call_id, "c1");
    assert_eq!(told.content, "5");
    assert!(!told.is_error);

    // A call without arguments, as models often send it.
    let told = tools.call(&call("c2", "shop_greet", "")).await;
    assert_eq!(told.content, "hello");

    server.abort();
}

/// Every failure reaches the model as a failed result it can act on, never as
/// an error of the caller's loop.
#[tokio::test(flavor = "multi_thread")]
async fn failures_are_told_to_the_model() {
    let (client, server) = serve(false).await;
    let tools = RemoteTools::new(client)
        .filter(|tool| tool.name != "greet")
        .load()
        .await
        .expect("load");

    let cases = [
        // The tool failed.
        (call("c1", "fail", "{}"), "out of paper"),
        // An image cannot be passed on, and is not dropped either.
        (call("c2", "photo", "{}"), "an image"),
        // Not offered: filtered out, or unknown.
        (call("c3", "greet", "{}"), "no tool named `greet`"),
        (call("c5", "nope", "{}"), "no tool named `nope`"),
        // Arguments that are not an object.
        (call("c6", "add", "[2, 3]"), "must be a JSON object"),
    ];
    for (made, expected) in cases {
        let told = tools.call(&made).await;
        assert_eq!(told.call_id, made.id);
        assert!(told.is_error, "{made:?} -> {told:?}");
        assert!(told.content.contains(expected), "{made:?} -> {told:?}");
    }

    // Hidden from the model by MCP Apps.
    #[cfg(feature = "apps")]
    {
        let told = tools.call(&call("c7", "cart", "{}")).await;
        assert!(told.is_error, "{told:?}");
        assert!(told.content.contains("no tool named `cart`"), "{told:?}");
    }

    server.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn calls_are_answered_in_order() {
    let (client, server) = serve(false).await;
    let tools = RemoteTools::new(client).load().await.expect("load");

    let calls: Vec<_> = (0..8)
        .map(|i| call(&format!("c{i}"), "add", &format!(r#"{{"a": {i}, "b": 1}}"#)))
        .collect();
    let told = tools.call_all(&calls).await;

    let answers: Vec<_> = told
        .iter()
        .map(|result| (result.call_id.as_str(), result.content.as_str()))
        .collect();
    let expected: Vec<_> = (0..8)
        .map(|i| (format!("c{i}"), (i + 1).to_string()))
        .collect();
    let expected: Vec<_> = expected
        .iter()
        .map(|(id, sum)| (id.as_str(), sum.as_str()))
        .collect();
    assert_eq!(answers, expected);

    server.abort();
}

/// A name a model API cannot carry is refused when the snapshot is taken,
/// not renamed behind the caller's back. Renaming it, or filtering it out, is
/// the caller's way past it.
#[tokio::test(flavor = "multi_thread")]
async fn a_name_a_model_cannot_carry_fails_the_load() {
    let (client, server) = serve(true).await;

    let err = RemoteTools::new(client.clone())
        .load()
        .await
        .expect_err("`files.read` cannot be offered");
    assert!(err.to_string().contains("files.read"), "{err}");

    let filtered = RemoteTools::new(client.clone())
        .filter(|tool| tool.name != "files.read")
        .load()
        .await
        .expect("filtered out");
    assert!(
        filtered
            .tools()
            .iter()
            .all(|tool| tool.name != "files.read")
    );

    // Renames add up in order, and the call reaches the server's name.
    let renamed = RemoteTools::new(client.clone())
        .rename(|name| name.replace('.', "_"))
        .prefixed("gh_")
        .load()
        .await
        .expect("renamed");
    assert!(
        renamed
            .tools()
            .iter()
            .any(|tool| tool.name == "gh_files_read")
    );
    let told = renamed.call(&call("c1", "gh_files_read", "")).await;
    assert_eq!(told.content, "contents", "{told:?}");

    // Two tools under one name cannot both be offered.
    let err = RemoteTools::new(client)
        .rename(|_| "same".into())
        .load()
        .await
        .expect_err("a name taken twice");
    assert!(err.to_string().contains("already taken"), "{err}");

    server.abort();
}

/// The toolbox can own its client outright; it stays reachable.
#[tokio::test(flavor = "multi_thread")]
async fn a_toolbox_can_own_its_client() {
    let (client, server) = serve(false).await;
    let client = Arc::into_inner(client).expect("the only handle");

    let tools = RemoteTools::new(client).load().await.expect("load");
    let listed = tools.client().tools().list_all().await.expect("tools/list");
    assert!(listed.iter().any(|tool| tool.name == "add"), "{listed:?}");

    server.abort();
}

/// A refresh takes the server's current tools, and only those can be called.
#[tokio::test(flavor = "multi_thread")]
async fn a_refresh_renews_the_snapshot() {
    let (client, server) = serve(false).await;
    let tools = RemoteTools::new(client)
        .filter(|tool| tool.name == "add")
        .load()
        .await
        .expect("load");
    assert_eq!(tools.tools().len(), 1);

    tools.refresh().await.expect("refresh");
    assert_eq!(tools.tools().len(), 1);
    assert_eq!(
        tools
            .call(&call("c1", "add", r#"{"a": 1, "b": 1}"#))
            .await
            .content,
        "2"
    );

    server.abort();
}

/// A prompt and a resource reach a conversation as messages and parts, read
/// through the client the toolbox owns.
#[tokio::test(flavor = "multi_thread")]
async fn prompts_and_resources_become_messages() {
    let (client, server) = serve(false).await;
    let tools = RemoteTools::new(client);

    let prompt = tools
        .client()
        .prompts()
        .get("review", ())
        .await
        .expect("prompts/get");
    let messages = neva::svir::prompt_messages(prompt).expect("messages");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, svir::Role::User);
    assert_eq!(
        messages[0].parts,
        [Part::Text("Review this change.".into())]
    );

    let notes = tools
        .client()
        .resources()
        .read("notes://monday")
        .await
        .expect("resources/read");
    assert_eq!(
        neva::svir::resource_parts(notes).expect("parts"),
        [Part::File(TextFile::text(
            "notes://monday",
            "Ship the bridge on monday."
        ))]
    );

    server.abort();
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

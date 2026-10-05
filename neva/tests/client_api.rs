//! The client's namespaced API against a real server.
//!
//! `client.tools()`, `client.resources()` and `client.prompts()` are views
//! over one shared client; `list_all` walks every page the server hands out.
#![cfg(all(feature = "http-server-volga", feature = "http-client"))]

use neva::{App, client::Client, types::Role};
use std::time::Duration;

/// More than two pages at the server's page size of ten.
const COUNT: usize = 25;

async fn serve() -> (String, tokio::task::JoinHandle<()>) {
    let addr = format!("127.0.0.1:{}", pick_free_port());

    let mut app = App::new()
        .without_greeting()
        .with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")));
    for i in 0..COUNT {
        app.map_tool(format!("tool-{i:02}"), move || async move {
            format!("tool {i}")
        });
        app.add_resource(format!("res://item/{i:02}"), format!("item-{i:02}"));
        app.map_prompt(format!("prompt-{i:02}"), move || async move {
            (format!("prompt {i}"), Role::User)
        });
    }

    let server = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    (addr, server)
}

#[tokio::test(flavor = "multi_thread")]
async fn list_all_walks_every_page() {
    let (addr, server) = serve().await;

    let mut client = Client::new().with_options(|o| {
        o.with_http(|h| h.bind(&addr).with_endpoint("/mcp"))
            .with_timeout(Duration::from_secs(10))
    });
    client.connect().await.expect("connect");

    let first = client.tools().list(None).await.expect("first page");
    assert!(
        first.next_cursor.is_some() && first.tools.len() < COUNT,
        "the server must page, or this test proves nothing"
    );

    // Views coexist: several at once, all over one shared borrow.
    let (tools, resources, prompts) = (client.tools(), client.resources(), client.prompts());

    let mut names: Vec<_> = tools
        .list_all()
        .await
        .expect("tools")
        .into_iter()
        .map(|tool| tool.name)
        .collect();
    names.sort();
    let expected: Vec<_> = (0..COUNT).map(|i| format!("tool-{i:02}")).collect();
    assert_eq!(names, expected);

    assert_eq!(resources.list_all().await.expect("resources").len(), COUNT);
    assert_eq!(prompts.list_all().await.expect("prompts").len(), COUNT);

    let result = tools.call("tool-07", ()).await.expect("call");
    let text = serde_json::to_string(&result).expect("serialize");
    assert!(text.contains("tool 7"), "{text}");

    server.abort();
}

/// The flat methods are deprecated spellings of the namespaced ones, and must
/// keep answering the same until they are removed.
#[tokio::test(flavor = "multi_thread")]
#[allow(deprecated)]
async fn deprecated_methods_still_answer() {
    let (addr, server) = serve().await;

    let mut client = Client::new().with_options(|o| {
        o.with_http(|h| h.bind(&addr).with_endpoint("/mcp"))
            .with_timeout(Duration::from_secs(10))
    });
    client.connect().await.expect("connect");

    assert_eq!(
        client.list_tools(None).await.expect("tools").tools.len(),
        client.tools().list(None).await.expect("tools").tools.len()
    );
    assert_eq!(
        client
            .list_prompts(None)
            .await
            .expect("prompts")
            .prompts
            .len(),
        client
            .prompts()
            .list(None)
            .await
            .expect("prompts")
            .prompts
            .len()
    );
    assert_eq!(
        client
            .list_resources(None)
            .await
            .expect("resources")
            .resources
            .len(),
        client
            .resources()
            .list(None)
            .await
            .expect("resources")
            .resources
            .len()
    );
    let result = client.call_tool("tool-03", ()).await.expect("call");
    assert!(
        serde_json::to_string(&result)
            .expect("json")
            .contains("tool 3")
    );

    server.abort();
}

/// Two batches in flight with the same caller-chosen request id would share
/// one response slot: the second would take over the first's waiter and be
/// handed the first's response. The second is refused instead, before anything
/// is sent, and the first still gets its own answer.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_id_already_in_flight_refuses_the_batch() {
    use neva::types::{MessageEnvelope, Request, RequestId};
    use std::sync::Arc;
    use tokio::sync::Notify;

    let addr = format!("127.0.0.1:{}", pick_free_port());
    let release = Arc::new(Notify::new());
    let mut app = App::new()
        .without_greeting()
        .with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")));
    let gate = release.clone();
    app.map_tool("hold", move || {
        let gate = gate.clone();
        async move {
            gate.notified().await;
            "held".to_string()
        }
    });
    app.map_tool("quick", || async move { "quick".to_string() });
    let server = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut client = Client::new().with_options(|o| {
        o.with_http(|h| h.bind(&addr).with_endpoint("/mcp"))
            .with_timeout(Duration::from_secs(10))
    });
    client.connect().await.expect("connect");
    let client = Arc::new(client);

    let call = |tool: &str| {
        MessageEnvelope::Request(Request::new(
            Some(RequestId::Number(777)),
            "tools/call",
            Some(serde_json::json!({ "name": tool, "arguments": {} })),
        ))
    };

    let first = {
        let client = client.clone();
        let item = call("hold");
        tokio::spawn(async move { client.call_batch(vec![item]).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;

    let err = client
        .call_batch(vec![call("quick")])
        .await
        .expect_err("id 777 is still waiting for the first batch's response");
    assert!(err.to_string().contains("777"), "{err}");

    release.notify_one();
    let responses = tokio::time::timeout(Duration::from_secs(5), first)
        .await
        .expect("the first batch must still be answered")
        .expect("task")
        .expect("batch");
    let text = serde_json::to_string(&responses).expect("serialize");
    assert!(
        text.contains("held"),
        "the first batch keeps its own response: {text}"
    );

    server.abort();
}

/// A batch its caller gave up on -- an outer `timeout`, a lost `select!`
/// branch -- gives its ids back at once. Held until the server answered or the
/// TTL ran out, they would refuse a retry of the same hand-built batch for that
/// whole time, against a server that may never answer.
#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_batch_gives_its_ids_back() {
    use neva::types::{MessageEnvelope, Request, RequestId};
    use std::sync::Arc;
    use tokio::sync::Notify;

    let addr = format!("127.0.0.1:{}", pick_free_port());
    let release = Arc::new(Notify::new());
    let mut app = App::new()
        .without_greeting()
        .with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")));
    let gate = release.clone();
    app.map_tool("hold", move || {
        let gate = gate.clone();
        async move {
            gate.notified().await;
            "held".to_string()
        }
    });
    app.map_tool("quick", || async move { "quick".to_string() });
    let server = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut client = Client::new().with_options(|o| {
        o.with_http(|h| h.bind(&addr).with_endpoint("/mcp"))
            // Far longer than this test waits: the ids must come back because
            // the batch was dropped, not because its requests timed out.
            .with_timeout(Duration::from_secs(30))
    });
    client.connect().await.expect("connect");

    let call = |id: i64, tool: &str| {
        MessageEnvelope::Request(Request::new(
            Some(RequestId::Number(id)),
            "tools/call",
            Some(serde_json::json!({ "name": tool, "arguments": {} })),
        ))
    };

    assert!(
        tokio::time::timeout(
            Duration::from_millis(300),
            client.call_batch(vec![call(881, "hold"), call(882, "hold")]),
        )
        .await
        .is_err(),
        "the server holds both calls, so the outer timeout must fire"
    );

    let responses = client
        .call_batch(vec![call(881, "quick"), call(882, "quick")])
        .await
        .expect("the abandoned batch gave its ids back");
    let text = serde_json::to_string(&responses).expect("serialize");
    assert!(text.contains("quick"), "{text}");

    release.notify_waiters();
    server.abort();
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

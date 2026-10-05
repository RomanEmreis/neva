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

type Gate = std::sync::Arc<tokio::sync::Notify>;

/// A server holding its answers to `hold` and `hold_b` until their gates are
/// released, and answering `quick` at once.
async fn serve_held() -> (String, Gate, Gate, tokio::task::JoinHandle<()>) {
    let addr = format!("127.0.0.1:{}", pick_free_port());
    let (release, release_b) = (Gate::default(), Gate::default());
    let mut app = App::new()
        .without_greeting()
        .with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")));
    for (name, gate, answer) in [("hold", &release, "held"), ("hold_b", &release_b, "held-b")] {
        let gate = gate.clone();
        app.map_tool(name, move || {
            let gate = gate.clone();
            async move {
                gate.notified().await;
                answer.to_string()
            }
        });
    }
    app.map_tool("quick", || async move { "quick".to_string() });
    let server = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    (addr, release, release_b, server)
}

fn call(id: i64, tool: &str) -> neva::types::MessageEnvelope {
    neva::types::MessageEnvelope::Request(neva::types::Request::new(
        Some(neva::types::RequestId::Number(id)),
        "tools/call",
        Some(serde_json::json!({ "name": tool, "arguments": {} })),
    ))
}

fn answers(responses: &[neva::types::Response]) -> Vec<(neva::types::RequestId, String)> {
    responses
        .iter()
        .map(|resp| {
            let text = serde_json::to_string(resp).expect("serialize");
            let answer = ["held-b", "held", "quick"]
                .into_iter()
                .find(|answer| text.contains(answer))
                .unwrap_or("none");
            (resp.id().clone(), answer.to_string())
        })
        .collect()
}

/// The client numbers every request it sends, a hand-built batch's included,
/// so two batches in flight under the same caller-chosen id do not share a
/// slot: each gets its own answer, under the id its caller gave.
#[tokio::test(flavor = "multi_thread")]
async fn batches_under_the_same_ids_get_their_own_answers() {
    use neva::types::RequestId;
    use std::sync::Arc;

    let (addr, release, _release_b, server) = serve_held().await;
    let mut client = Client::new().with_options(|o| {
        o.with_http(|h| h.bind(&addr).with_endpoint("/mcp"))
            .with_timeout(Duration::from_secs(10))
    });
    client.connect().await.expect("connect");
    let client = Arc::new(client);

    let first = {
        let client = client.clone();
        tokio::spawn(async move { client.call_batch(vec![call(777, "hold")]).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;

    let second = client
        .call_batch(vec![call(777, "quick")])
        .await
        .expect("the second batch is its own request on the wire");
    assert_eq!(
        answers(&second),
        [(RequestId::Number(777), "quick".to_string())]
    );

    release.notify_one();
    let first = tokio::time::timeout(Duration::from_secs(5), first)
        .await
        .expect("the first batch must still be answered")
        .expect("task")
        .expect("batch");
    assert_eq!(
        answers(&first),
        [(RequestId::Number(777), "held".to_string())]
    );

    server.abort();
}

/// A batch its caller gave up on -- an outer `timeout`, a lost `select!`
/// branch -- frees its slots at once, and its late answers have nowhere to go.
/// A retry under the same caller ids is new requests on the wire: while it is
/// still waiting, the abandoned batch's answers arrive and must not be taken
/// for its own.
#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_batch_leaves_nothing_for_its_retry() {
    use neva::types::RequestId;
    use std::sync::Arc;

    let (addr, release, release_b, server) = serve_held().await;
    let mut client = Client::new().with_options(|o| {
        o.with_http(|h| h.bind(&addr).with_endpoint("/mcp"))
            .with_timeout(Duration::from_secs(30))
    });
    client.connect().await.expect("connect");
    let client = Arc::new(client);

    assert!(
        tokio::time::timeout(
            Duration::from_millis(300),
            client.call_batch(vec![call(881, "hold"), call(882, "hold")]),
        )
        .await
        .is_err(),
        "the server holds both calls, so the outer timeout must fire"
    );

    let retry = {
        let client = client.clone();
        tokio::spawn(async move {
            client
                .call_batch(vec![call(881, "hold_b"), call(882, "hold_b")])
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;

    // The abandoned batch's answers arrive while the retry still waits.
    release.notify_waiters();
    tokio::time::sleep(Duration::from_millis(200)).await;
    release_b.notify_waiters();

    let retry = tokio::time::timeout(Duration::from_secs(5), retry)
        .await
        .expect("the retry must be answered")
        .expect("task")
        .expect("batch");
    assert_eq!(
        answers(&retry),
        [
            (RequestId::Number(881), "held-b".to_string()),
            (RequestId::Number(882), "held-b".to_string()),
        ],
        "the retry gets its own answers, not the abandoned batch's"
    );

    server.abort();
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

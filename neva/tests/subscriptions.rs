//! `subscriptions/listen` end-to-end over the 2026-07-28 HTTP transport.
//!
//! The listen `POST` stays open as a `text/event-stream`: the acknowledgment
//! arrives first, then only the notification types the filter opted in to, each
//! tagged with the subscription id, and finally the graceful-close result once
//! the client cancels.
#![cfg(all(
    not(feature = "legacy-spec"),
    feature = "http-server-volga",
    feature = "http-client"
))]

use neva::App;
use neva::types::{SUBSCRIPTION_ID_KEY, Tool};
use std::time::Duration;

const RESOURCE: &str = "res://watched";

#[tokio::test(flavor = "multi_thread")]
async fn subscription_streams_only_the_requested_notifications() {
    let addr = format!("127.0.0.1:{}", pick_free_port());
    let mut app = App::new().with_options(|opt| {
        opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
            .with_tools(|t| t.with_list_changed())
            .with_prompts(|p| p.with_list_changed())
            .with_resources(|r| r.with_list_changed().with_subscribe())
    });
    // Mutations a client can trigger, each producing one subscribable
    // notification.
    app.map_tool("grow", |mut ctx: neva::Context| async move {
        ctx.add_tool(Tool::new("grown", || async { "ok" })).await?;
        Ok::<_, neva::error::Error>("grown".to_string())
    });
    app.map_tool("touch", |mut ctx: neva::Context| async move {
        ctx.resource_updated(RESOURCE).await?;
        Ok::<_, neva::error::Error>("touched".to_string())
    });
    app.map_tool("add_prompt", |mut ctx: neva::Context| async move {
        ctx.add_prompt(neva::types::Prompt::new("fresh", || async {
            neva::types::PromptMessage::user().with("hi")
        }))
        .await?;
        Ok::<_, neva::error::Error>("added".to_string())
    });

    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("test client");
    let url = format!("http://{addr}/mcp");

    // Open the subscription: tools list changes plus updates for one resource.
    // Prompts are deliberately left out -- the server must never send them.
    let listen = serde_json::json!({
        "jsonrpc": "2.0", "id": "sub-1", "method": "subscriptions/listen",
        "params": {
            "notifications": {
                "toolsListChanged": true,
                "resourceSubscriptions": [RESOURCE]
            },
            "_meta": meta()
        }
    });
    let mut stream = routed(client.post(&url), &listen)
        .header("Accept", "application/json, text/event-stream")
        .json(&listen)
        .send()
        .await
        .expect("listen failed");

    assert!(stream.status().is_success());
    let ctype = content_type(&stream);
    assert!(
        ctype.contains("text/event-stream"),
        "a subscription reply must be a stream, got content-type {ctype:?}"
    );

    // The acknowledgment MUST be the first message, carrying the accepted
    // subset and the subscription id.
    let mut body = String::new();
    let ack = next_message(&mut stream, &mut body).await;
    assert_eq!(ack["method"], "notifications/subscriptions/acknowledged");
    assert_eq!(ack["params"]["_meta"][SUBSCRIPTION_ID_KEY], "sub-1");
    assert_eq!(ack["params"]["notifications"]["toolsListChanged"], true);
    assert_eq!(
        ack["params"]["notifications"]["resourceSubscriptions"][0],
        RESOURCE
    );
    assert!(
        ack["params"]["notifications"]
            .get("promptsListChanged")
            .is_none(),
        "the acknowledgment must omit types the client never requested"
    );

    // A mutation on another request reaches this stream...
    call_tool(&client, &url, "add_prompt", 2).await;
    call_tool(&client, &url, "grow", 3).await;

    let notification = next_message(&mut stream, &mut body).await;
    assert_eq!(
        notification["method"], "notifications/tools/list_changed",
        "the prompts notification must not appear: it was never requested"
    );
    assert_eq!(
        notification["params"]["_meta"][SUBSCRIPTION_ID_KEY],
        "sub-1"
    );

    // ...and so does an update for a subscribed resource.
    call_tool(&client, &url, "touch", 4).await;

    let updated = next_message(&mut stream, &mut body).await;
    assert_eq!(updated["method"], "notifications/resources/updated");
    assert_eq!(updated["params"]["uri"], RESOURCE);
    assert_eq!(updated["params"]["_meta"][SUBSCRIPTION_ID_KEY], "sub-1");

    // Cancelling ends the stream with the graceful-close result.
    let cancel = serde_json::json!({
        "jsonrpc": "2.0", "method": "notifications/cancelled",
        "params": { "requestId": "sub-1", "reason": "done" }
    });
    routed(client.post(&url), &cancel)
        .header("Accept", "application/json, text/event-stream")
        .json(&cancel)
        .send()
        .await
        .expect("cancel failed");

    let closed = next_message(&mut stream, &mut body).await;
    assert_eq!(closed["id"], "sub-1");
    assert_eq!(closed["result"]["_meta"][SUBSCRIPTION_ID_KEY], "sub-1");
    assert_eq!(closed["result"]["resultType"], "complete");

    handle.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn subscription_is_narrowed_to_advertised_capabilities() {
    // A server that never advertises `listChanged` cannot promise it: the
    // acknowledgment reports an empty filter rather than refusing the stream.
    let addr = format!("127.0.0.1:{}", pick_free_port());
    let mut app =
        App::new().with_options(|opt| opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp")));
    app.map_tool("noop", || async { "ok" });

    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("test client");
    let url = format!("http://{addr}/mcp");

    let listen = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "subscriptions/listen",
        "params": {
            "notifications": { "toolsListChanged": true, "promptsListChanged": true },
            "_meta": meta()
        }
    });
    let mut stream = routed(client.post(&url), &listen)
        .header("Accept", "application/json, text/event-stream")
        .json(&listen)
        .send()
        .await
        .expect("listen failed");

    let mut body = String::new();
    let ack = next_message(&mut stream, &mut body).await;

    assert_eq!(ack["method"], "notifications/subscriptions/acknowledged");
    assert_eq!(ack["params"]["_meta"][SUBSCRIPTION_ID_KEY], 1);
    assert_eq!(
        ack["params"]["notifications"],
        serde_json::json!({}),
        "an unadvertised type must be dropped from the accepted filter"
    );

    handle.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn client_listen_delivers_to_registered_handlers() {
    // The neva client's half of the same round trip: `listen` returns once the
    // filter is acknowledged, and notifications land on the handlers registered
    // with `on_tools_changed` and friends.
    use neva::Client;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let addr = format!("127.0.0.1:{}", pick_free_port());
    let mut app = App::new().with_options(|opt| {
        opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
            .with_tools(|t| t.with_list_changed())
    });
    app.map_tool("grow", |mut ctx: neva::Context| async move {
        ctx.add_tool(Tool::new("grown", || async { "ok" })).await?;
        Ok::<_, neva::error::Error>("grown".to_string())
    });

    let handle = tokio::spawn(async move { app.run().await });
    await_reachable(&addr).await;

    let seen = Arc::new(AtomicUsize::new(0));
    let counter = seen.clone();

    let mut client = Client::new().with_options(|opt| {
        opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
            .with_timeout(Duration::from_secs(5))
    });
    client.connect().await.expect("connect");
    // After `connect`: the helper asserts the server advertises `listChanged`,
    // which is only known once capabilities have been discovered.
    client.on_tools_changed(move |_| {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });

    let subscription = client
        .listen(neva::types::SubscriptionFilter::new().with_tools_changed())
        .await
        .expect("listen");

    assert!(subscription.acknowledged().tools_list_changed);
    assert!(subscription.is_fully_honored());

    client.call_tool("grow", ()).await.expect("tools/call");

    // The notification travels on the listen stream, so it arrives out of band
    // from the call's own reply.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while seen.load(Ordering::SeqCst) == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        seen.load(Ordering::SeqCst),
        1,
        "the registered handler must see the tools list change"
    );

    handle.abort();
}

async fn await_reachable(addr: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match tokio::net::TcpStream::connect(addr).await {
            Ok(_) => break,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(50)).await
            }
            Err(err) => panic!("server never became reachable: {err}"),
        }
    }
}

/// Calls a tool over a plain (non-streaming) POST and waits for its reply, so
/// the mutation it performs has definitely happened.
async fn call_tool(client: &reqwest::Client, url: &str, name: &str, id: i64) {
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": { "name": name, "arguments": {}, "_meta": meta() }
    });
    let resp = routed(client.post(url), &call)
        .header("Accept", "application/json, text/event-stream")
        .json(&call)
        .send()
        .await
        .unwrap_or_else(|e| panic!("{name} call failed: {e}"));
    assert!(resp.status().is_success(), "{name} call was rejected");
    let _ = resp.text().await;
}

/// Pulls chunks off a live SSE body until one more complete `data:` frame is
/// available, and returns it parsed.
///
/// `body` accumulates across calls: an SSE chunk boundary does not have to line
/// up with a frame boundary, so a partially-read frame must survive to the next
/// call.
async fn next_message(resp: &mut reqwest::Response, body: &mut String) -> serde_json::Value {
    loop {
        if let Some(msg) = take_frame(body) {
            return msg;
        }
        let chunk = tokio::time::timeout(Duration::from_secs(5), resp.chunk())
            .await
            .expect("timed out waiting for the next subscription message")
            .expect("stream error")
            .expect("stream ended before the expected message");
        body.push_str(&String::from_utf8_lossy(&chunk));
    }
}

/// Removes and parses the first complete `data:` frame in `body`, if any.
fn take_frame(body: &mut String) -> Option<serde_json::Value> {
    let end = body.find("\n\n")?;
    let frame: String = body.drain(..end + 2).collect();
    frame
        .lines()
        .find_map(|line| line.strip_prefix("data:"))
        .and_then(|data| serde_json::from_str(data.trim()).ok())
}

fn content_type(resp: &reqwest::Response) -> String {
    resp.headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// The `_meta` MCP 2026-07-28 requires on every request.
fn meta() -> serde_json::Value {
    serde_json::json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {}
    })
}

/// Attaches the routing headers MCP 2026-07-28 requires on every request.
fn routed(req: reqwest::RequestBuilder, body: &serde_json::Value) -> reqwest::RequestBuilder {
    let req = req.header("MCP-Protocol-Version", "2026-07-28");
    let Some(method) = body["method"].as_str() else {
        return req;
    };
    let req = req.header("Mcp-Method", method);
    match method {
        "tools/call" => match body.pointer("/params/name").and_then(|v| v.as_str()) {
            Some(name) => req.header("Mcp-Name", name),
            None => req,
        },
        _ => req,
    }
}

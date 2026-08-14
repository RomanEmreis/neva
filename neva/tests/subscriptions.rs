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

    // Over HTTP a `notifications/cancelled` naming a bare id must NOT end the
    // stream: it arrives on its own POST and proves nothing about who opened
    // the subscription, while ids collide across clients routinely.
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

    call_tool(&client, &url, "grow", 5).await;
    let still_live = next_message(&mut stream, &mut body).await;
    assert_eq!(
        still_live["method"], "notifications/tools/list_changed",
        "a bare cancel must not end a session-bound subscription"
    );

    // Closing the stream is the mechanism the spec names on this transport.
    drop(stream);
    tokio::time::sleep(Duration::from_millis(200)).await;

    // The server survives it and keeps serving: the subscription was torn down,
    // not the runtime.
    call_tool(&client, &url, "grow", 6).await;

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

#[tokio::test(flavor = "multi_thread")]
async fn client_cancel_ends_the_stream_over_http() {
    // `Subscription::cancel` has to actually end the subscription on this
    // transport, where the server cannot act on a bare `notifications/cancelled`
    // -- the client closes its listen response body instead, and the handler
    // observes that through its sink.
    use neva::Client;
    use neva::client::SubscriptionEnd;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let addr = format!("127.0.0.1:{}", pick_free_port());
    let mut app = App::new().with_options(|opt| {
        opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
            .with_tools(|t| t.with_list_changed())
    });
    app.map_tool("grow", |mut ctx: neva::Context| async move {
        ctx.add_tool(Tool::new(
            format!("grown-{}", uuid::Uuid::new_v4()),
            || async { "ok" },
        ))
        .await?;
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
    client.on_tools_changed(move |_| {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });

    let mut subscription = client
        .listen(neva::types::SubscriptionFilter::new().with_tools_changed())
        .await
        .expect("listen");

    client.call_tool("grow", ()).await.expect("first mutation");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while seen.load(Ordering::SeqCst) == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(seen.load(Ordering::SeqCst), 1, "the stream must be live");

    subscription.cancel().await.expect("cancel");
    // Cancelling closes the stream, so no final result comes back -- and
    // `closed()` must say so instead of waiting one out.
    let ended = tokio::time::timeout(Duration::from_secs(2), subscription.closed())
        .await
        .expect("closed() must not hang after a cancel");
    assert!(matches!(ended, SubscriptionEnd::Cancelled), "got {ended:?}");

    // Nothing arrives after the cancel.
    tokio::time::sleep(Duration::from_millis(200)).await;
    client.call_tool("grow", ()).await.expect("second mutation");
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert_eq!(
        seen.load(Ordering::SeqCst),
        1,
        "a cancelled subscription must stop delivering"
    );

    handle.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_handle_ends_the_subscription() {
    // A `Subscription` that falls out of scope without `cancel()` must not leave
    // the peer streaming: there is no handle left to stop it, but its
    // notifications would still reach the client's registered handlers.
    use neva::Client;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let addr = format!("127.0.0.1:{}", pick_free_port());
    let mut app = App::new().with_options(|opt| {
        opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
            .with_tools(|t| t.with_list_changed())
    });
    app.map_tool("grow", |mut ctx: neva::Context| async move {
        ctx.add_tool(Tool::new(
            format!("grown-{}", uuid::Uuid::new_v4()),
            || async { "ok" },
        ))
        .await?;
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
    client.on_tools_changed(move |_| {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });

    {
        let _subscription = client
            .listen(neva::types::SubscriptionFilter::new().with_tools_changed())
            .await
            .expect("listen");

        client.call_tool("grow", ()).await.expect("first mutation");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while seen.load(Ordering::SeqCst) == 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(seen.load(Ordering::SeqCst), 1, "the stream must be live");
    } // <- handle dropped here, with no cancel() and no closed()

    tokio::time::sleep(Duration::from_millis(300)).await;
    client.call_tool("grow", ()).await.expect("second mutation");
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert_eq!(
        seen.load(Ordering::SeqCst),
        1,
        "a dropped subscription must stop delivering"
    );

    handle.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn disconnecting_ends_the_subscription_abruptly() {
    // Disconnecting takes the transport out from under a live subscription:
    // client-side its final result can never arrive, so `closed()` has to say
    // `Abrupt` rather than await one that is not coming; server-side the listen
    // POST has to close, or the subscription outlives the client that opened it.
    use neva::Client;
    use neva::client::SubscriptionEnd;

    let addr = format!("127.0.0.1:{}", pick_free_port());
    let mut app = App::new().with_options(|opt| {
        opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
            .with_tools(|t| t.with_list_changed())
            .with_resources(|r| r.with_list_changed().with_subscribe())
    });
    app.map_tool("watched", |ctx: neva::Context| async move {
        Ok::<_, neva::error::Error>(ctx.is_subscribed(&"res://config".into()).to_string())
    });

    let handle = tokio::spawn(async move { app.run().await });
    await_reachable(&addr).await;

    let mut observer = Client::new().with_options(|opt| {
        opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
            .with_timeout(Duration::from_secs(5))
    });
    observer.connect().await.expect("observer connect");

    let mut client = Client::new().with_options(|opt| {
        opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
            .with_timeout(Duration::from_secs(5))
    });
    client.connect().await.expect("connect");

    let subscription = client
        .listen(neva::types::SubscriptionFilter::new().with_resource("res://config"))
        .await
        .expect("listen");
    assert_eq!(
        watched(&mut observer).await,
        Some("true".into()),
        "the server must see the subscription while it is live"
    );

    client.disconnect().await.expect("disconnect");

    let ended = tokio::time::timeout(Duration::from_secs(5), subscription.closed())
        .await
        .expect("closed() must not hang once the transport is gone");
    assert!(matches!(ended, SubscriptionEnd::Abrupt), "got {ended:?}");

    // The listen POST closing is what tells the server the subscription is
    // over. Without it the entry stays registered and the server goes on
    // broadcasting into a client that disconnected.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if watched(&mut observer).await.as_deref() == Some("false") {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the subscription outlived the client that opened it"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    handle.abort();
}

/// Asks the server whether anything is currently listening for `res://config`.
async fn watched(client: &mut neva::Client) -> Option<String> {
    let resp = client.call_tool("watched", ()).await.expect("watched call");
    resp.content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.to_string())
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

// -- Cross-instance fan-out -------------------------------------------------

/// A `subscriptions/listen` stream is a socket held open by one process, and
/// the stateless transport pins nothing to an instance: the subscriber and the
/// request that mutates the server routinely land on different ones. A
/// `NotificationBus` is what carries the notification across.
#[tokio::test(flavor = "multi_thread")]
async fn a_notification_bus_delivers_across_instances() {
    let (tx, _) = tokio::sync::broadcast::channel(64);
    let bus = BroadcastBus(tx);

    let addr_a = format!("127.0.0.1:{}", pick_free_port());
    let addr_b = format!("127.0.0.1:{}", pick_free_port());
    let a = tokio::spawn(instance(&addr_a, Some(bus.clone())).run());
    let b = tokio::spawn(instance(&addr_b, Some(bus)).run());
    await_reachable(&addr_a).await;
    await_reachable(&addr_b).await;

    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("test client");

    // The subscription lives on instance A...
    let (mut stream, mut body) = listen(&client, &addr_a).await;

    // ...and every mutation happens on instance B, which holds no subscribers
    // at all.
    let url_b = format!("http://{addr_b}/mcp");
    call_tool(&client, &url_b, "grow", 2).await;

    let notification = next_message(&mut stream, &mut body).await;
    assert_eq!(
        notification["method"], "notifications/tools/list_changed",
        "a mutation on another instance must reach this subscription"
    );
    assert_eq!(
        notification["params"]["_meta"][SUBSCRIPTION_ID_KEY],
        "sub-1"
    );

    // `resources/updated` is the one that used to be gated on a node-local
    // "is anybody watching?" check, which on instance B answers `false`.
    call_tool(&client, &url_b, "touch", 3).await;

    let updated = next_message(&mut stream, &mut body).await;
    assert_eq!(updated["method"], "notifications/resources/updated");
    assert_eq!(updated["params"]["uri"], RESOURCE);
    assert_eq!(updated["params"]["_meta"][SUBSCRIPTION_ID_KEY], "sub-1");

    a.abort();
    b.abort();
}

/// The other half of the proof: without a bus the same two instances lose the
/// notification, which is the behaviour the bus exists to fix. It also pins the
/// default -- an instance delivers to its own subscribers and to nobody else's.
#[tokio::test(flavor = "multi_thread")]
async fn without_a_bus_a_notification_stays_on_its_own_instance() {
    let addr_a = format!("127.0.0.1:{}", pick_free_port());
    let addr_b = format!("127.0.0.1:{}", pick_free_port());
    let a = tokio::spawn(instance(&addr_a, None).run());
    let b = tokio::spawn(instance(&addr_b, None).run());
    await_reachable(&addr_a).await;
    await_reachable(&addr_b).await;

    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("test client");

    let (mut stream, mut body) = listen(&client, &addr_a).await;

    call_tool(&client, &format!("http://{addr_b}/mcp"), "grow", 2).await;
    assert!(
        no_message(&mut stream, &mut body).await,
        "instance B cannot write into instance A's stream"
    );

    // The same mutation on the instance holding the stream still arrives, so
    // the wait above timed out for the right reason.
    call_tool(&client, &format!("http://{addr_a}/mcp"), "grow", 3).await;
    let notification = next_message(&mut stream, &mut body).await;
    assert_eq!(notification["method"], "notifications/tools/list_changed");

    a.abort();
    b.abort();
}

/// Stands in for a shared bus (Redis pub/sub, NATS, Postgres `LISTEN/NOTIFY`):
/// one process-wide channel every instance publishes to and reads back from,
/// its own messages included -- which is what the trait's no-echo-suppression
/// rule asks for.
#[derive(Clone)]
struct BroadcastBus(tokio::sync::broadcast::Sender<(String, Option<serde_json::Value>)>);

impl neva::NotificationBus for BroadcastBus {
    fn publish<'a>(
        &'a self,
        method: &'a str,
        params: Option<&'a serde_json::Value>,
    ) -> neva::shared::BoxFuture<'a, ()> {
        let msg = (method.to_owned(), params.cloned());
        Box::pin(async move {
            // No receiver yet simply means no instance is draining, which is
            // not an error worth failing the request that produced it.
            let _ = self.0.send(msg);
        })
    }

    fn subscribe(&self) -> neva::shared::BoxStream<'static, (String, Option<serde_json::Value>)> {
        let rx = self.0.subscribe();
        Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
            loop {
                match rx.recv().await {
                    Ok(msg) => return Some((msg, rx)),
                    // At-most-once: a lagging drain skips what it missed rather
                    // than ending delivery for good.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => return None,
                }
            }
        }))
    }
}

/// One instance of the same logical server: the tools are identical on both,
/// only the address and the bus differ.
fn instance(addr: &str, bus: Option<BroadcastBus>) -> App {
    let mut app = App::new().with_options(|opt| {
        opt.with_http(|http| http.bind(addr).with_endpoint("/mcp"))
            .with_tools(|t| t.with_list_changed())
            .with_resources(|r| r.with_list_changed().with_subscribe())
    });
    if let Some(bus) = bus {
        app = app.with_notification_bus(bus);
    }
    app.map_tool("grow", |mut ctx: neva::Context| async move {
        ctx.add_tool(Tool::new("grown", || async { "ok" })).await?;
        Ok::<_, neva::error::Error>("grown".to_string())
    });
    app.map_tool("touch", |mut ctx: neva::Context| async move {
        ctx.resource_updated(RESOURCE).await?;
        Ok::<_, neva::error::Error>("touched".to_string())
    });
    app
}

/// Opens a subscription against `addr` and consumes its acknowledgment, so the
/// caller's next `next_message` is the first real notification.
async fn listen(client: &reqwest::Client, addr: &str) -> (reqwest::Response, String) {
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
    let mut stream = routed(client.post(format!("http://{addr}/mcp")), &listen)
        .header("Accept", "application/json, text/event-stream")
        .json(&listen)
        .send()
        .await
        .expect("listen failed");
    assert!(stream.status().is_success());

    let mut body = String::new();
    let ack = next_message(&mut stream, &mut body).await;
    assert_eq!(ack["method"], "notifications/subscriptions/acknowledged");
    (stream, body)
}

/// Returns whether the stream stays silent for long enough to call it silent.
///
/// There is no positive signal for "nothing will arrive", so this is a bounded
/// wait; the test that uses it follows up with a delivery that *must* arrive,
/// which is what rules out a stream that was simply broken.
async fn no_message(resp: &mut reqwest::Response, body: &mut String) -> bool {
    tokio::time::timeout(Duration::from_secs(1), next_message(resp, body))
        .await
        .is_err()
}

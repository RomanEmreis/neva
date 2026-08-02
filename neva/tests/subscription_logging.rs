//! What a `subscriptions/listen` response body may carry (MCP 2026-07-28).
//!
//! The body of a listen `POST` is the subscription's stream: the
//! acknowledgment MUST be its first message. Request-scoped
//! `notifications/message` share the same body, and middleware wrapped around
//! the handler emits them *before* the handler ever runs -- so they are held
//! back until the acknowledgment goes out, then released. Held, not dropped:
//! the client asked for them, and in a mixed batch they may belong to another
//! request entirely.
//!
//! Its own file because the notification layer is installed as the process-wide
//! default subscriber.
#![cfg(all(
    not(feature = "legacy-spec"),
    feature = "http-server-volga",
    feature = "http-client",
    feature = "tracing"
))]

use neva::App;
use neva::types::notification;
use std::time::Duration;
use tracing_subscriber::prelude::*;

const MARKER: &str = "logged-before-next";

#[tokio::test(flavor = "multi_thread")]
async fn a_listen_stream_opens_with_its_acknowledgment() {
    install_subscriber();

    let addr = format!("127.0.0.1:{}", pick_free_port());
    let mut app = App::new()
        .with_options(|opt| {
            opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
                .with_tools(|t| t.with_list_changed())
        })
        // Logs *before* `next(ctx)`: by the time the listen handler queues its
        // acknowledgment, this event has already been through the layer. The
        // per-request sink exists from the moment the transport takes the
        // request, so nothing but the stream's own rules keeps it off the body.
        .wrap(|ctx, next| async move {
            tracing::warn!(logger = "mw", "{MARKER}");
            next(ctx).await
        });
    app.map_tool("grow", |mut ctx: neva::Context| async move {
        ctx.add_tool(neva::types::Tool::new(
            format!("grown-{}", uuid::Uuid::new_v4()),
            || async { "ok" },
        ))
        .await?;
        Ok::<_, neva::error::Error>("grown".to_string())
    });

    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("test client");
    let url = format!("http://{addr}/mcp");

    let listen = serde_json::json!({
        "jsonrpc": "2.0", "id": "sub-1", "method": "subscriptions/listen",
        "params": {
            "notifications": { "toolsListChanged": true },
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/logLevel": "info"
            }
        }
    });
    let mut stream = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "subscriptions/listen")
        .header("Accept", "application/json, text/event-stream")
        .json(&listen)
        .send()
        .await
        .expect("listen failed");

    assert!(stream.status().is_success());

    let mut body = String::new();
    let first = next_message(&mut stream, &mut body).await;
    assert_eq!(
        first["method"], "notifications/subscriptions/acknowledged",
        "the acknowledgment must be the first message on a subscription stream, got {first}"
    );

    // The log that preceded it is not lost -- it follows the acknowledgment
    // rather than jumping ahead of it.
    let logged = next_message(&mut stream, &mut body).await;
    assert_eq!(
        logged["method"], "notifications/message",
        "the held log message must be released after the acknowledgment, got {logged}"
    );
    assert_eq!(logged["params"]["data"]["message"], MARKER);

    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "grow", "arguments": {}, "_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {}
        } }
    });
    let resp = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "grow")
        .header("Accept", "application/json, text/event-stream")
        .json(&call)
        .send()
        .await
        .expect("grow failed");
    let _ = resp.text().await;

    let next = next_message(&mut stream, &mut body).await;
    assert_eq!(
        next["method"], "notifications/tools/list_changed",
        "the subscription's own notification must follow, got {next}"
    );

    // A listen inside a batch streams on the same kind of body under the same
    // rule. neva's own client refuses to batch one -- there would be no handle
    // to end the subscription with -- but this server takes what any peer
    // sends, so the classification has to see through the batch.
    let batched = serde_json::json!([{
        "jsonrpc": "2.0", "id": "sub-2", "method": "subscriptions/listen",
        "params": {
            "notifications": { "toolsListChanged": true },
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/logLevel": "info"
            }
        }
    }]);
    let mut batch_stream = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        // No `Mcp-Method`: the server rejects one on a batch, since it cannot
        // describe several requests at once.
        .header("Accept", "application/json, text/event-stream")
        .json(&batched)
        .send()
        .await
        .expect("batched listen failed");

    assert!(batch_stream.status().is_success());

    let mut batch_body = String::new();
    let first = next_message(&mut batch_stream, &mut batch_body).await;
    assert_eq!(
        first["method"], "notifications/subscriptions/acknowledged",
        "a batched listen streams under the same rule, got {first}"
    );

    handle.abort();
}

/// The acknowledgment coming first is a MUST; the held logs riding along after
/// it are an accommodation. A handler that floods the body before the listen
/// handler runs must therefore lose the overflow, not the ordering.
#[tokio::test(flavor = "multi_thread")]
async fn a_flood_before_the_acknowledgment_never_displaces_it() {
    const HELD: usize = 2;
    const FLOOD: usize = 8;

    install_subscriber();

    let addr = format!("127.0.0.1:{}", pick_free_port());
    let mut app = App::new()
        .with_options(|opt| {
            opt.with_http(|http| {
                http.bind(&addr)
                    .with_endpoint("/mcp")
                    .with_sse_log_queue(HELD)
            })
            .with_tools(|t| t.with_list_changed())
        })
        // Paced, not bursted: the stream drains each message into the hold
        // buffer before the next arrives, so the buffer outgrows the channel
        // while the channel itself never fills -- which is the only way to
        // reach the limit with the acknowledgment still able to get through.
        .wrap(|ctx, next| async move {
            for i in 0..FLOOD {
                tracing::warn!(logger = "mw", "{MARKER}-{i}");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            next(ctx).await
        });
    app.map_tool("grow", |mut ctx: neva::Context| async move {
        ctx.add_tool(neva::types::Tool::new(
            format!("grown-{}", uuid::Uuid::new_v4()),
            || async { "ok" },
        ))
        .await?;
        Ok::<_, neva::error::Error>("grown".to_string())
    });

    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("test client");
    let url = format!("http://{addr}/mcp");

    let listen = serde_json::json!({
        "jsonrpc": "2.0", "id": "sub-1", "method": "subscriptions/listen",
        "params": {
            "notifications": { "toolsListChanged": true },
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/logLevel": "info"
            }
        }
    });
    let mut stream = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "subscriptions/listen")
        .header("Accept", "application/json, text/event-stream")
        .json(&listen)
        .send()
        .await
        .expect("listen failed");

    let mut body = String::new();
    let first = next_message(&mut stream, &mut body).await;
    assert_eq!(
        first["method"], "notifications/subscriptions/acknowledged",
        "a flood before the acknowledgment must not displace it, got {first}"
    );

    // What is released after it is bounded: the buffer holds what the sink
    // itself would have, and the rest is dropped rather than reordered.
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "grow", "arguments": {}, "_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {}
        } }
    });
    let resp = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "grow")
        .header("Accept", "application/json, text/event-stream")
        .json(&call)
        .send()
        .await
        .expect("grow failed");
    let _ = resp.text().await;

    let mut released = 0;
    loop {
        let msg = next_message(&mut stream, &mut body).await;
        match msg["method"].as_str().unwrap_or_default() {
            "notifications/message" => released += 1,
            "notifications/tools/list_changed" => break,
            other => panic!("unexpected frame on the subscription stream: {other}"),
        }
    }
    assert!(
        released <= HELD,
        "the pre-acknowledgment buffer must stay bounded, got {released} of {FLOOD}"
    );

    handle.abort();
}

/// The acknowledgment's room on the body is reserved before any middleware can
/// run, so a burst that fills the sink outright cannot cost the subscription
/// its handshake -- it only costs the logs that did not fit, which is what a
/// bounded sink does anyway.
#[tokio::test(flavor = "multi_thread")]
async fn a_burst_that_fills_the_sink_still_opens_the_subscription() {
    install_subscriber();

    let addr = format!("127.0.0.1:{}", pick_free_port());
    let mut app = App::new()
        .with_options(|opt| {
            opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp").with_sse_log_queue(1))
                .with_tools(|t| t.with_list_changed())
        })
        // Synchronous, so nothing drains between them: with a capacity of one,
        // the channel is full by the time the listen handler runs.
        .wrap(|ctx, next| async move {
            for i in 0..16 {
                tracing::warn!(logger = "mw", "{MARKER}-{i}");
            }
            next(ctx).await
        });
    app.map_tool("ping", || async { "pong" });

    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("test client");
    let url = format!("http://{addr}/mcp");

    let listen = serde_json::json!({
        "jsonrpc": "2.0", "id": "sub-1", "method": "subscriptions/listen",
        "params": {
            "notifications": { "toolsListChanged": true },
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/logLevel": "info"
            }
        }
    });
    let mut stream = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "subscriptions/listen")
        .header("Accept", "application/json, text/event-stream")
        .json(&listen)
        .send()
        .await
        .expect("listen failed");

    let mut body = String::new();
    let first = next_message(&mut stream, &mut body).await;
    assert_eq!(
        first["method"], "notifications/subscriptions/acknowledged",
        "a full sink must not cost the subscription its acknowledgment, got {first}"
    );

    handle.abort();
}

/// The notification layer is the process-wide subscriber, so whichever test
/// gets there first installs it -- both want the same one.
fn install_subscriber() {
    let _ = tracing_subscriber::registry()
        .with(tracing::level_filters::LevelFilter::WARN)
        .with(notification::fmt::layer())
        .try_init();
}

/// Pulls chunks off a live SSE body until one more complete `data:` frame is
/// available, and returns it parsed.
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

fn take_frame(body: &mut String) -> Option<serde_json::Value> {
    let end = body.find("\n\n")?;
    let frame: String = body.drain(..end + 2).collect();
    frame
        .lines()
        .find_map(|line| line.strip_prefix("data:"))
        .and_then(|data| serde_json::from_str(data.trim()).ok())
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

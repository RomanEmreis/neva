//! Request-scoped logging over the RC stateless HTTP transport (MCP 2026-07-28).
//!
//! A `POST` whose `_meta` carries `io.modelcontextprotocol/logLevel` gets a
//! `text/event-stream` reply carrying the request's `notifications/message`
//! followed by the response; a `POST` without it gets a plain JSON reply and no
//! log notifications (the spec's suppression rule).
#![cfg(all(
    feature = "proto-2026-07-28-rc",
    feature = "http-server-volga",
    feature = "http-client",
    feature = "tracing"
))]

use neva::App;
use neva::types::notification;
use tracing_subscriber::prelude::*;

const MARKER: &str = "distinctive-log-marker";

#[tokio::test(flavor = "multi_thread")]
async fn request_scoped_logging_streams_over_post() {
    // The notification layer must be the active subscriber in the server task so
    // the tool's `tracing` events become `notifications/message`. Each test file
    // is its own process, so installing the global default here is safe.
    tracing_subscriber::registry()
        .with(notification::fmt::layer())
        .init();

    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app =
        App::new().with_options(|opt| opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp")));
    app.map_tool("shout", || async move {
        tracing::warn!(logger = "tool", "{MARKER}");
        "pong".to_string()
    });
    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/mcp");

    // (a) opted in via `_meta.logLevel` -> SSE reply with the log then response.
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "name": "shout",
            "arguments": {},
            "_meta": { "io.modelcontextprotocol/logLevel": "info" }
        }
    });
    let resp = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Accept", "application/json, text/event-stream")
        .json(&call)
        .send()
        .await
        .expect("logged call failed");
    assert!(resp.status().is_success());
    let ctype = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(
        ctype.contains("text/event-stream"),
        "opted-in reply must be an SSE stream, got content-type {ctype:?}"
    );
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("notifications/message"),
        "SSE body must carry the log notification, got: {body}"
    );
    assert!(
        body.contains(MARKER),
        "SSE body must carry the log message, got: {body}"
    );
    assert!(
        body.contains("pong"),
        "SSE body must carry the response, got: {body}"
    );

    // (b) no `logLevel` -> plain JSON reply, no log notifications (suppressed).
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "shout", "arguments": {} }
    });
    let resp = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Accept", "application/json, text/event-stream")
        .json(&call)
        .send()
        .await
        .expect("plain call failed");
    assert!(resp.status().is_success());
    let ctype = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(
        !ctype.contains("text/event-stream"),
        "reply without logLevel must be plain JSON, got content-type {ctype:?}"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body.pointer("/result/content/0/text")
            .and_then(|v| v.as_str()),
        Some("pong")
    );

    // (c) a batch whose inner request opts in streams too: the inner request's
    // log rides the single SSE POST response alongside the batch response.
    let batch = serde_json::json!([
        {
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {
                "name": "shout",
                "arguments": {},
                "_meta": { "io.modelcontextprotocol/logLevel": "info" }
            }
        },
        {
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": { "name": "shout", "arguments": {} }
        }
    ]);
    let resp = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Accept", "application/json, text/event-stream")
        .json(&batch)
        .send()
        .await
        .expect("batch call failed");
    assert!(resp.status().is_success());
    let ctype = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(
        ctype.contains("text/event-stream"),
        "a batch with an opted-in inner request must stream, got content-type {ctype:?}"
    );
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("notifications/message") && body.contains(MARKER),
        "batch SSE body must carry the inner request's log, got: {body}"
    );

    handle.abort();
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

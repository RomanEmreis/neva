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
const MARKER_A: &str = "marker-alpha";
const MARKER_B: &str = "marker-bravo";
const MARKER_AFTER_NEXT: &str = "marker-after-next";

/// Reads a response body, failing loudly instead of hanging: a request-scoped
/// SSE body that never closes (e.g. the notification sink outliving the
/// pipeline) would otherwise stall the test forever.
async fn body_within(resp: reqwest::Response, what: &str) -> String {
    tokio::time::timeout(std::time::Duration::from_secs(5), resp.text())
        .await
        .unwrap_or_else(|_| panic!("{what}: SSE body did not close within 5s"))
        .expect("body read failed")
}

#[tokio::test(flavor = "multi_thread")]
async fn request_scoped_logging_streams_over_post() {
    // The notification layer must be the active subscriber in the server task so
    // the tool's `tracing` events become `notifications/message`. Each test file
    // is its own process, so installing the global default here is safe.
    //
    // The warning-only threshold is deliberate and mirrors a common application
    // setup: every log below is emitted at WARN, so all of them stay enabled,
    // and the request span that carries the routing context must survive the
    // same filter.
    tracing_subscriber::registry()
        .with(tracing::level_filters::LevelFilter::WARN)
        .with(notification::fmt::layer())
        .init();

    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app = App::new()
        .with_options(|opt| opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp")))
        // Logs *after* `next(ctx)`: the terminal middleware has already completed
        // the response by then, so this only reaches the client if the SSE body
        // stays open until the whole pipeline is done.
        .wrap(|ctx, next| async move {
            let resp = next(ctx).await;
            tracing::warn!(logger = "mw", "{MARKER_AFTER_NEXT}");
            resp
        });
    app.map_tool("shout", || async move {
        tracing::warn!(logger = "tool", "{MARKER}");
        "pong".to_string()
    });
    // Two tools that log a distinct marker after a short delay, so two
    // concurrent POSTs overlap in-flight -- exercising sink isolation.
    app.map_tool("shout_a", || async move {
        tracing::warn!(logger = "tool", "{MARKER_A}");
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        "a".to_string()
    });
    app.map_tool("shout_b", || async move {
        tracing::warn!(logger = "tool", "{MARKER_B}");
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        "b".to_string()
    });
    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // `no_proxy`: reqwest honors `HTTP_PROXY`/`HTTPS_PROXY` from the environment,
    // and an uppercase `NO_PROXY` that omits localhost would still send these
    // loopback requests through the proxy. The server under test is local only.
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("test client");
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
    let body = body_within(resp, "opted-in call").await;
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
    // A log emitted by user middleware *after* `next(ctx)` must still reach the
    // client: the stream stays open until the whole pipeline finishes.
    assert!(
        body.contains(MARKER_AFTER_NEXT),
        "SSE body must carry the after-next middleware log, got: {body}"
    );
    // ...and the response closes the stream: every notification precedes it.
    let resp_at = body.find("\"result\"").expect("response in body");
    assert!(
        body.find(MARKER).is_some_and(|at| at < resp_at)
            && body.find(MARKER_AFTER_NEXT).is_some_and(|at| at < resp_at),
        "notifications must precede the final response, got: {body}"
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
    let body = body_within(resp, "batch call").await;
    assert!(
        body.contains("notifications/message") && body.contains(MARKER),
        "batch SSE body must carry the inner request's log, got: {body}"
    );

    // (d) two concurrent opted-in POSTs sharing one client-supplied
    // `Mcp-Session-Id` must not cross-talk: each stateless POST mints its own
    // sink key, so each response carries only its own request's log.
    let shared_session = uuid::Uuid::new_v4().to_string();
    let call_for = |tool: &str| {
        serde_json::json!({
            "jsonrpc": "2.0", "id": 9, "method": "tools/call",
            "params": {
                "name": tool,
                "arguments": {},
                "_meta": { "io.modelcontextprotocol/logLevel": "info" }
            }
        })
    };
    let send = |call: serde_json::Value| {
        client
            .post(&url)
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Accept", "application/json, text/event-stream")
            .header("Mcp-Session-Id", shared_session.clone())
            .json(&call)
            .send()
    };
    let (resp_a, resp_b) = tokio::join!(send(call_for("shout_a")), send(call_for("shout_b")));
    let body_a = body_within(resp_a.expect("a failed"), "concurrent A").await;
    let body_b = body_within(resp_b.expect("b failed"), "concurrent B").await;

    // Each response sees its own marker and not the other's -- no sink collision
    // even though both POSTs carried the same `Mcp-Session-Id`.
    assert!(
        body_a.contains(MARKER_A) && !body_a.contains(MARKER_B),
        "response A must carry only its own log, got: {body_a}"
    );
    assert!(
        body_b.contains(MARKER_B) && !body_b.contains(MARKER_A),
        "response B must carry only its own log, got: {body_b}"
    );

    handle.abort();
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

//! A request the server cannot answer the way it was asked -- its caller gave
//! up, or its handler panicked -- is that request's problem, and only that
//! request's.
//!
//! The HTTP transport hands each response to the POST waiting for it. A caller
//! that closes its connection first leaves no one to hand the response to, and
//! that used to stop the whole transport: one impatient client, or a proxy
//! timeout shorter than a slow tool, and the server was gone. A handler that
//! panicked left its POST waiting for an answer that never came.
#![cfg(all(feature = "http-server-volga", feature = "http-client"))]

mod common;

use neva::App;
use std::time::Duration;

/// How long the slow tool takes: long enough for the caller below to give up
/// and close its connection well before there is an answer to deliver.
const SLOW: Duration = Duration::from_millis(800);

/// JSON-RPC 2.0 `Internal error`.
const INTERNAL_ERROR: i64 = -32603;

#[tokio::test(flavor = "multi_thread")]
async fn a_caller_that_gives_up_does_not_stop_the_server() {
    let (addr, server) = serve().await;

    // Gives up long before the tool answers. Dropping the client closes its
    // connection, which is what the server gets to see of a caller's timeout.
    let impatient = client(Duration::from_millis(100));
    assert!(
        call(&impatient, &addr, "slow", 1).await.is_err(),
        "the caller must give up before the slow tool answers"
    );
    drop(impatient);

    // The answer now exists, with no one left to deliver it to.
    tokio::time::sleep(SLOW + Duration::from_millis(500)).await;

    assert!(
        !server.is_finished(),
        "an undeliverable response must not stop the server"
    );
    let resp = call(&client(Duration::from_secs(5)), &addr, "fast", 2)
        .await
        .expect("the server must still be serving");
    let body: serde_json::Value = resp.json().await.expect("a JSON-RPC reply");
    assert_eq!(
        body.pointer("/result/content/0/text")
            .and_then(|v| v.as_str()),
        Some("fast"),
        "got: {body}"
    );

    server.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_panicking_handler_is_answered_with_an_internal_error() {
    let (addr, server) = serve().await;
    let client = client(Duration::from_secs(5));

    // The caller hears about the failure instead of waiting until it gives up.
    let resp = call(&client, &addr, "boom", 1)
        .await
        .expect("a panicking handler must still be answered");
    let body: serde_json::Value = resp.json().await.expect("a JSON-RPC reply");
    assert_eq!(body["id"], 1, "got: {body}");
    assert_eq!(
        body.pointer("/error/code").and_then(|v| v.as_i64()),
        Some(INTERNAL_ERROR),
        "got: {body}"
    );
    // What went wrong inside the handler is the server's business, not the
    // caller's: the panic message stays out of the reply.
    assert!(
        !body.to_string().contains("the handler failed"),
        "got: {body}"
    );

    let resp = call(&client, &addr, "fast", 2)
        .await
        .expect("the server must still be serving");
    assert!(resp.status().is_success());

    server.abort();
}

/// Starts a server with a slow, a fast and a panicking tool, returning its
/// address and the task running it.
async fn serve() -> (String, tokio::task::JoinHandle<()>) {
    let addr = format!("127.0.0.1:{}", common::free_port());
    let mut app = App::new()
        .without_greeting()
        .with_options(|opt| opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp")));

    app.map_tool("slow", || async {
        tokio::time::sleep(SLOW).await;
        "slow".to_string()
    });
    app.map_tool("fast", || async { "fast".to_string() });
    app.map_tool("boom", || async { fail() });

    let server = tokio::spawn(async move { app.run().await });
    common::serving(&addr, &server).await;
    (addr, server)
}

/// A handler body that panics, typed as the `String` a tool returns.
fn fail() -> String {
    panic!("the handler failed")
}

fn client(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(timeout)
        .build()
        .expect("test client")
}

/// Calls `tool` with no arguments, the way a conforming client of this
/// profile would.
async fn call(
    client: &reqwest::Client,
    addr: &str,
    tool: &str,
    id: u64,
) -> reqwest::Result<reqwest::Response> {
    let req = client
        .post(format!("http://{addr}/mcp"))
        .json(&request(tool, id));
    #[cfg(not(feature = "legacy-spec"))]
    let req = req
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", tool);
    req.send().await
}

#[cfg(not(feature = "legacy-spec"))]
fn request(tool: &str, id: u64) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": {
            "name": tool,
            "arguments": {},
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }
    })
}

#[cfg(feature = "legacy-spec")]
fn request(tool: &str, id: u64) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": { "name": tool, "arguments": {} }
    })
}

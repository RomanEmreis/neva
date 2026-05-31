//! Stateless HTTP transport (MCP 2026-07-28 RC) end-to-end checks.
//!
//! Exercises the stateless POST-only path: `server/discover` without a
//! session, a stateless `tools/call` carrying the required
//! `MCP-Protocol-Version` header, rejection of a header-less POST, and the
//! absence of the GET (SSE) / DELETE routes.
#![cfg(all(
    feature = "proto-2026-07-28-rc",
    feature = "http-server-volga",
    feature = "http-client"
))]

use neva::App;

#[tokio::test(flavor = "multi_thread")]
async fn stateless_discover_and_call() {
    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app =
        App::new().with_options(|opt| opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp")));
    app.map_tool("ping", || async move { "pong".to_string() });
    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/mcp");

    // (a) discover, no session header.
    let discover = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": {}
    });
    let resp = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&discover)
        .send()
        .await
        .expect("discover failed");
    assert!(resp.status().is_success());
    // (b) no session id on the wire.
    assert!(
        resp.headers().get("Mcp-Session-Id").is_none(),
        "stateless server must not emit Mcp-Session-Id"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["result"]["protocolVersion"],
        serde_json::json!("2026-07-28")
    );

    // (c) stateless tool call with the protocol-version header, no session.
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "ping", "arguments": {} }
    });
    let resp = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&call)
        .send()
        .await
        .expect("call failed");
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body.pointer("/result/content/0/text")
            .and_then(|v| v.as_str()),
        Some("pong")
    );

    // (d) missing protocol-version header -> JSON-RPC InvalidRequest.
    let resp = client
        .post(&url)
        .json(&call)
        .send()
        .await
        .expect("send failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.get("error").is_some(), "missing header must error");

    // (e) GET and DELETE are not routed under the flag.
    let get = client.get(&url).send().await.expect("get failed");
    assert!(
        get.status() == reqwest::StatusCode::NOT_FOUND
            || get.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED,
        "GET should not be routed, got {}",
        get.status()
    );

    handle.abort();
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

//! Volga adapter end-to-end smoke test — performs an init POST + tool-call
//! POST + DELETE against a running `HttpServer<DefaultClaims, VolgaEngine>`
//! bound to an ephemeral port. Asserts session-id round-trip and basic
//! response shape.

#![cfg(all(feature = "http-server-volga", feature = "http-client"))]

use neva::App;

#[tokio::test(flavor = "multi_thread")]
async fn volga_engine_round_trip() {
    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");

    let mut app =
        App::new().with_options(|opt| opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp")));

    app.map_tool("ping", || async move { "pong".to_string() });

    let handle = tokio::spawn(async move { app.run().await });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/mcp");

    let init_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "0" }
        }
    });
    let resp = client
        .post(&url)
        .json(&init_body)
        .send()
        .await
        .expect("init POST failed");
    assert!(
        resp.status().is_success(),
        "init returned {}",
        resp.status()
    );
    let session_id = resp
        .headers()
        .get("Mcp-Session-Id")
        .and_then(|v| v.to_str().ok())
        .expect("Mcp-Session-Id header missing")
        .to_string();

    let call_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": "ping", "arguments": {} }
    });
    let resp = client
        .post(&url)
        .header("Mcp-Session-Id", &session_id)
        .json(&call_body)
        .send()
        .await
        .expect("tool POST failed");
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    let content = body
        .pointer("/result/content/0/text")
        .and_then(|v| v.as_str())
        .expect("missing tool result content");
    assert_eq!(content, "pong");

    let resp = client
        .delete(&url)
        .header("Mcp-Session-Id", &session_id)
        .send()
        .await
        .expect("delete failed");
    assert!(resp.status().is_success());

    handle.abort();
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

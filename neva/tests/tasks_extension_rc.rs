//! Tasks-as-extension (MCP 2026-07-28 RC) end-to-end checks.
//!
//! Under the RC flag the tasks capability is advertised through the extensions
//! map (`capabilities.extensions["io.modelcontextprotocol/tasks"]`) instead of
//! the former top-level `capabilities.tasks` field, while the `tasks/*` wire
//! methods are unchanged. Exercised over the stateless POST-only path.
#![cfg(all(
    feature = "proto-2026-07-28-rc",
    feature = "tasks",
    feature = "http-server-volga",
    feature = "http-client"
))]

use neva::App;

#[tokio::test(flavor = "multi_thread")]
async fn tasks_capability_is_advertised_as_extension() {
    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app = App::new().with_options(|opt| {
        opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
            .with_tasks(|t| t.with_all())
    });
    app.map_tool("ping", || async move { "pong".to_string() });
    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/mcp");

    // (a) discover advertises tasks under the extensions map, not top-level.
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
    let body: serde_json::Value = resp.json().await.unwrap();
    let caps = &body["result"]["capabilities"];

    assert!(
        caps["extensions"]["io.modelcontextprotocol/tasks"].is_object(),
        "tasks must be advertised under capabilities.extensions, got: {caps}"
    );
    assert!(
        caps.get("tasks").is_none(),
        "no top-level capabilities.tasks under the RC flag, got: {caps}"
    );

    // (b) the tasks/* wire methods are unchanged and still dispatch.
    let list = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tasks/list", "params": {}
    });
    let resp = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&list)
        .send()
        .await
        .expect("tasks/list failed");
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["result"]["tasks"].is_array(),
        "tasks/list should return a task array, got: {body}"
    );

    handle.abort();
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

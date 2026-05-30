//! MRTR (elicitation) end-to-end over the stateless RC transport.
//!
//! Drives the raw protocol so the two-round wire contract is asserted
//! directly: round 1 `tools/call` → `input_required` (+ `requestState`),
//! round 2 retry (new id + `inputResponses` + echoed state) → final result.
#![cfg(all(
    feature = "proto-2026-07-28-rc",
    feature = "http-server-volga",
    feature = "http-client"
))]

use neva::{App, Context, error::Error, types::elicitation::ElicitRequestParams};

#[tokio::test(flavor = "multi_thread")]
async fn tool_elicits_then_completes_over_two_rounds() {
    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app = App::new()
        .with_request_state_secret(b"test-secret")
        .with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")));

    app.map_tool("greet", |mut ctx: Context| async move {
        let params: ElicitRequestParams = ElicitRequestParams::form("Your name?")
            .with_required("name", "string")
            .into();
        let res = ctx.elicit("name", params).await?;
        let name = res
            .content
            .and_then(|c| c.get("name").and_then(|v| v.as_str().map(str::to_owned)))
            .unwrap_or_else(|| "stranger".into());
        Ok::<String, Error>(format!("hello {name}"))
    });

    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/mcp");

    // Round 1: tools/call → input_required.
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "greet", "arguments": {},
            "_meta": { "clientCapabilities": { "elicitation": true } } }
    });
    let r1: serde_json::Value = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&call)
        .send()
        .await
        .expect("round 1 send")
        .json()
        .await
        .expect("round 1 json");
    assert_eq!(
        r1["result"]["resultType"],
        serde_json::json!("input_required"),
        "round 1 must request input: {r1}"
    );
    let state = r1["result"]["requestState"]
        .as_str()
        .expect("requestState present")
        .to_string();
    let key = r1["result"]["inputRequests"]
        .as_object()
        .expect("inputRequests object")
        .keys()
        .next()
        .expect("one input request")
        .clone();

    // Round 2: retry with a new id + inputResponses + echoed state.
    let retry = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "greet", "arguments": {},
            "_meta": {
                "clientCapabilities": { "elicitation": true },
                "requestState": state,
                "inputResponses": { key: { "action": "accept", "content": { "name": "octocat" } } }
            } }
    });
    let r2: serde_json::Value = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&retry)
        .send()
        .await
        .expect("round 2 send")
        .json()
        .await
        .expect("round 2 json");
    assert_eq!(
        r2.pointer("/result/content/0/text")
            .and_then(|v| v.as_str()),
        Some("hello octocat"),
        "round 2 must complete: {r2}"
    );

    handle.abort();
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

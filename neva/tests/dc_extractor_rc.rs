//! RC end-to-end check that the `Dc<T>` DI extractor is **not** advertised as a
//! tool/prompt argument and is resolved from the request context at call time.
//!
//! Regression test: `get_arg_type` used to classify the unknown `Dc<_>` type as
//! `"object"`, so the `#[neva::tool]` / `#[neva::prompt]` macros listed the
//! injected dependency as a required input argument (clients then had to send a
//! bogus `repo` field). Resources were unaffected because their schema is
//! derived from the URI template, not the function signature.
//!
//! Compiled under `proto-2026-07-28-rc` together with the Volga server + HTTP
//! client and `di` (all pulled in by `server-full` / `client-full`).

#![cfg(all(
    feature = "proto-2026-07-28-rc",
    feature = "http-server-volga",
    feature = "http-client",
    feature = "di"
))]

use neva::App;
use neva::di::Dc;

struct Counter {
    value: i64,
}

impl Counter {
    fn get(&self) -> i64 {
        self.value
    }
}

// Tool with ONLY an injected dependency: nothing should be advertised as an
// input argument, and the dependency must resolve at call time.
#[neva::tool]
async fn read_counter(counter: Dc<Counter>) -> String {
    counter.get().to_string()
}

// Tool mixing a real primitive arg with an injected dependency: only `delta`
// must be advertised; `counter` must be silently injected.
#[neva::tool]
async fn add_to_counter(delta: i64, counter: Dc<Counter>) -> String {
    (counter.get() + delta).to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn dc_extractor_is_injected_not_advertised() {
    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");

    let app = App::new()
        .with_options(|opt| opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp")))
        .add_singleton(Counter { value: 41 });
    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/mcp");

    // --- tools/list: assert the Dc dependency is not advertised ---
    let list_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    });
    let resp = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&list_body)
        .send()
        .await
        .expect("tools/list failed");
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    let tools = body
        .pointer("/result/tools")
        .and_then(|v| v.as_array())
        .expect("missing tools array");

    let by_name = |name: &str| -> serde_json::Value {
        tools
            .iter()
            .find(|t| t["name"] == serde_json::json!(name))
            .unwrap_or_else(|| panic!("tool {name} not listed"))
            .clone()
    };

    // read_counter: the `counter: Dc<_>` arg must not appear at all.
    let read = by_name("read_counter");
    let props = &read["inputSchema"]["properties"];
    assert!(
        props.get("counter").is_none(),
        "Dc dependency must not be advertised as a property: {read}"
    );
    let required = read["inputSchema"]["required"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !required.iter().any(|v| v == "counter"),
        "Dc dependency must not be required: {read}"
    );

    // add_to_counter: only `delta` is advertised, never `counter`.
    let add = by_name("add_to_counter");
    let add_props = &add["inputSchema"]["properties"];
    assert!(
        add_props.get("delta").is_some(),
        "real arg `delta` must be advertised: {add}"
    );
    assert!(
        add_props.get("counter").is_none(),
        "Dc dependency must not be advertised alongside real args: {add}"
    );
    let add_required: Vec<String> =
        serde_json::from_value(add["inputSchema"]["required"].clone()).unwrap_or_default();
    assert_eq!(
        add_required,
        vec!["delta".to_string()],
        "only the real arg must be required"
    );

    // --- tools/call: the Dc dependency must be injected and usable ---
    let call_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": "read_counter" }
    });
    let resp = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&call_body)
        .send()
        .await
        .expect("tools/call read_counter failed");
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    let text = body
        .pointer("/result/content/0/text")
        .and_then(|v| v.as_str())
        .expect("missing tool result text");
    assert_eq!(text, "41", "Dc<Counter> was not resolved from context");

    // Call with the real arg too: dependency injected, arg deserialized.
    let call_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": { "name": "add_to_counter", "arguments": { "delta": 1 } }
    });
    let resp = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&call_body)
        .send()
        .await
        .expect("tools/call add_to_counter failed");
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    let text = body
        .pointer("/result/content/0/text")
        .and_then(|v| v.as_str())
        .expect("missing tool result text");
    assert_eq!(text, "42", "Dc<Counter> + real arg did not both resolve");

    handle.abort();
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

//! RC-only end-to-end check that the `#[neva::tool]` macro emits valid JSON
//! Schema 2020-12 `inputSchema` / `outputSchema`. Compiled only under
//! `proto-2026-07-28-rc` together with the Volga server + HTTP client (both
//! pulled in by `server-full` / `client-full`). This is the sole `#[tool]`
//! call-site compiled in the RC CI configuration.

#![cfg(all(
    feature = "proto-2026-07-28-rc",
    feature = "http-server-volga",
    feature = "http-client"
))]

use neva::App;
use neva::types::Json;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct Profile {
    name: String,
    age: u32,
}

// No `JsonSchema` derive — must degrade to `{"type":"object"}`.
#[derive(Deserialize)]
#[allow(dead_code)]
struct Opaque {
    blob: String,
}

#[derive(Serialize, schemars::JsonSchema)]
struct Greeting {
    message: String,
}

// Primitive args -> inline primitive property schemas.
#[neva::tool]
async fn add(a: i32, b: i32) -> i32 {
    a + b
}

// Structured `Json<T>` arg whose inner type derives JsonSchema -> rich inlined
// object schema (the macro unwraps `Json<_>` and probes the inner type).
#[neva::tool]
async fn save_profile(profile: Json<Profile>) -> String {
    profile.0.name
}

// Structured `Json<T>` arg whose inner type lacks JsonSchema -> fallback object.
#[neva::tool]
async fn store(payload: Json<Opaque>) -> String {
    payload.0.blob
}

// Explicit input schema string (valid JSON).
#[neva::tool(input_schema = r#"{"type":"object","properties":{"q":{"type":"string"}},"required":["q"]}"#)]
async fn search(q: String) -> String {
    q
}

// Struct return via `Json<T>` -> output schema derived from the return type.
#[neva::tool]
async fn make_greeting(name: String) -> Json<Greeting> {
    Json(Greeting { message: format!("hi {name}") })
}

#[tokio::test(flavor = "multi_thread")]
async fn tool_macro_emits_json_schema_2020() {
    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");

    let app = App::new()
        .with_options(|opt| opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp")));
    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/mcp");

    let init_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2026-07-28",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "0" }
        }
    });
    let resp = client.post(&url).json(&init_body).send().await.expect("init failed");
    assert!(resp.status().is_success(), "init returned {}", resp.status());
    let session_id = resp
        .headers()
        .get("Mcp-Session-Id")
        .and_then(|v| v.to_str().ok())
        .expect("Mcp-Session-Id header missing")
        .to_string();

    let list_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });
    let resp = client
        .post(&url)
        .header("Mcp-Session-Id", &session_id)
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

    // 1. Primitive args -> object schema with primitive properties + required.
    let add = by_name("add");
    assert_eq!(add["inputSchema"]["type"], serde_json::json!("object"));
    assert_eq!(add["inputSchema"]["properties"]["a"]["type"], serde_json::json!("number"));
    assert_eq!(add["inputSchema"]["properties"]["b"]["type"], serde_json::json!("number"));
    let req: Vec<String> =
        serde_json::from_value(add["inputSchema"]["required"].clone()).unwrap();
    assert!(req.contains(&"a".to_string()) && req.contains(&"b".to_string()));

    // 2. Custom arg deriving JsonSchema -> rich, inlined (no $defs/$ref).
    let save = by_name("save_profile");
    let profile_schema = &save["inputSchema"]["properties"]["profile"];
    assert_eq!(profile_schema["type"], serde_json::json!("object"));
    assert!(profile_schema["properties"]["name"].is_object());
    assert!(profile_schema["properties"]["age"].is_object());
    let save_str = serde_json::to_string(&save["inputSchema"]).unwrap();
    assert!(!save_str.contains("$ref"), "must be inlined: {save_str}");
    assert!(!save_str.contains("$defs"), "must be inlined: {save_str}");

    // 3. Custom arg WITHOUT JsonSchema -> opaque object fallback.
    let store = by_name("store");
    assert_eq!(
        store["inputSchema"]["properties"]["payload"],
        serde_json::json!({ "type": "object" })
    );

    // 4. Explicit input schema string round-trips.
    let search = by_name("search");
    assert_eq!(search["inputSchema"]["properties"]["q"]["type"], serde_json::json!("string"));
    let req: Vec<String> =
        serde_json::from_value(search["inputSchema"]["required"].clone()).unwrap();
    assert_eq!(req, vec!["q".to_string()]);

    // 5a. Primitive (`String`) return -> no output schema (parity).
    assert!(
        by_name("save_profile")["outputSchema"].is_null(),
        "primitive return must not emit outputSchema"
    );

    // 5b. `Json<Greeting>` return -> output schema derived from `Greeting`.
    let greet = by_name("make_greeting");
    assert_eq!(greet["outputSchema"]["type"], serde_json::json!("object"));
    assert!(greet["outputSchema"]["properties"]["message"].is_object());

    handle.abort();
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

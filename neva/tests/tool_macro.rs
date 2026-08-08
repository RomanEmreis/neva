//! 2026-07-28-only end-to-end check that the `#[neva::tool]` macro emits valid JSON
//! Schema 2020-12 `inputSchema` / `outputSchema`. Compiled only under
//! MCP 2026-07-28 together with the Volga server + HTTP client (both
//! pulled in by `server-full` / `client-full`). This is the sole `#[tool]`
//! call-site compiled in the default CI configuration.

#![cfg(all(
    not(feature = "legacy-spec"),
    feature = "server-macros",
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

// No `JsonSchema` derive -- must degrade to `{"type":"object"}`.
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
#[neva::tool(
    input_schema = r#"{"type":"object","properties":{"q":{"type":"string"}},"required":["q"]}"#
)]
async fn search(q: String) -> String {
    q
}

// Two arguments of *different* types: the pair that used to fail outright when
// a map iteration order handed them to the handler the wrong way round.
#[neva::tool]
async fn describe(name: String, age: i32) -> String {
    format!("{name} is {age}")
}

// An optional argument: published as its inner type, kept out of `required`,
// and `None` when the call leaves it out.
#[neva::tool]
async fn nickname(name: String, alias: Option<String>) -> String {
    alias.unwrap_or(name)
}

// An optional structured argument still probes past both wrappers for a rich
// subschema.
#[neva::tool]
async fn maybe_profile(profile: Option<Json<Profile>>) -> String {
    profile.map(|p| p.0.name).unwrap_or_default()
}

// A metadata parameter reaching the signature through a type alias. The macro
// cannot see through the alias, so both the schema and the declared argument
// names have to be settled by trait resolution -- otherwise the tool publishes
// a `token` argument its handler never reads, and `App::run` refuses to start
// on the disagreement.
type Progress = neva::types::Meta<neva::types::ProgressToken>;

#[neva::tool]
async fn aliased(token: Progress, city: String) -> String {
    let _ = token;
    city
}

// Struct return via `Json<T>` -> output schema derived from the return type.
#[neva::tool]
async fn make_greeting(name: String) -> Json<Greeting> {
    Json(Greeting {
        message: format!("hi {name}"),
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn tool_macro_emits_json_schema_2020() {
    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");

    let app =
        App::new().with_options(|opt| opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp")));
    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("test client");
    let url = format!("http://{addr}/mcp");

    // Stateless 2026-07-28 transport: no handshake/session -- a single `tools/list`
    // POST carrying the required `MCP-Protocol-Version` header is enough.
    let list_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": { "_meta": meta() }
    });
    let resp = routed(client.post(&url), &list_body)
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
    assert_eq!(
        add["inputSchema"]["properties"]["a"]["type"],
        serde_json::json!("number")
    );
    assert_eq!(
        add["inputSchema"]["properties"]["b"]["type"],
        serde_json::json!("number")
    );
    let req: Vec<String> = serde_json::from_value(add["inputSchema"]["required"].clone()).unwrap();
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
    assert_eq!(
        search["inputSchema"]["properties"]["q"]["type"],
        serde_json::json!("string")
    );
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

    // 6. An optional argument is published like any other but is not required,
    // and a structured one is still described past both wrappers.
    let nickname = by_name("nickname");
    assert_eq!(
        nickname["inputSchema"]["properties"]["alias"]["type"],
        serde_json::json!("string")
    );
    let req: Vec<String> =
        serde_json::from_value(nickname["inputSchema"]["required"].clone()).unwrap();
    assert_eq!(
        req,
        vec!["name".to_string()],
        "`alias` must not be required"
    );

    let maybe = by_name("maybe_profile");
    let profile_schema = &maybe["inputSchema"]["properties"]["profile"];
    assert_eq!(profile_schema["type"], serde_json::json!("object"));
    assert!(
        profile_schema["properties"]["name"].is_object(),
        "an Option<Json<T>> arg must still describe T: {profile_schema}"
    );
    assert!(
        maybe["inputSchema"]["required"].is_null(),
        "an all-optional tool requires nothing"
    );

    // 7. An aliased metadata parameter is neither published nor named.
    let aliased = by_name("aliased");
    let props = aliased["inputSchema"]["properties"].as_object().unwrap();
    assert_eq!(
        props.keys().collect::<Vec<_>>(),
        vec!["city"],
        "an aliased `Meta<_>` must not be published: {aliased}"
    );
    let req: Vec<String> =
        serde_json::from_value(aliased["inputSchema"]["required"].clone()).unwrap();
    assert_eq!(req, vec!["city".to_string()]);

    // 8. An optional argument the call leaves out arrives as `None`.
    for (args, expected) in [
        (serde_json::json!({ "name": "John" }), "John"),
        (
            serde_json::json!({ "name": "John", "alias": "Johnny" }),
            "Johnny",
        ),
    ] {
        let call_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": { "name": "nickname", "arguments": args, "_meta": meta() }
        });
        let resp = routed(client.post(&url), &call_body)
            .json(&call_body)
            .send()
            .await
            .expect("tools/call failed");
        let body: serde_json::Value = resp.json().await.unwrap();

        assert_eq!(
            body.pointer("/result/content/0/text"),
            Some(&serde_json::json!(expected)),
            "unexpected response: {body}"
        );
    }

    // 9. Arguments are read by name, so the order a peer happens to serialize
    // them in cannot reach the handler. JSON object members are unordered and
    // both spellings below are the same call.
    for args in [
        serde_json::json!({ "name": "John", "age": 30 }),
        serde_json::json!({ "age": 30, "name": "John" }),
    ] {
        let call_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": "describe", "arguments": args, "_meta": meta() }
        });
        let resp = routed(client.post(&url), &call_body)
            .json(&call_body)
            .send()
            .await
            .expect("tools/call failed");
        let body: serde_json::Value = resp.json().await.unwrap();

        assert_eq!(
            body.pointer("/result/content/0/text"),
            Some(&serde_json::json!("John is 30")),
            "unexpected response: {body}"
        );
    }

    handle.abort();
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// The `_meta` MCP 2026-07-28 requires on every request: the protocol version,
/// and the capabilities this request is made under -- empty being the valid
/// declaration of "no optional capabilities".
fn meta() -> serde_json::Value {
    serde_json::json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {}
    })
}

/// Attaches the routing headers MCP 2026-07-28 requires on every request, the
/// way a conforming client derives them: from the body it is about to send.
fn routed(req: reqwest::RequestBuilder, body: &serde_json::Value) -> reqwest::RequestBuilder {
    let method = body["method"].as_str().unwrap_or_default();
    let req = req
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", method);
    let name = match method {
        "tools/call" | "prompts/get" => body.pointer("/params/name"),
        "resources/read" => body.pointer("/params/uri"),
        "tasks/get" | "tasks/update" | "tasks/cancel" => body.pointer("/params/taskId"),
        _ => None,
    };
    match name.and_then(|v| v.as_str()) {
        Some(name) => req.header("Mcp-Name", name),
        None => req,
    }
}

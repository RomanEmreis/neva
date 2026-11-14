//! The `Dc<T>` extractor check of `dc_extractor.rs`, driven through
//! `neva::testing` instead of hand-built HTTP requests: no port, no raw JSON,
//! the same assertions.
//!
//! `dc_extractor.rs` stays as the check over the real HTTP transport.

#![cfg(all(
    not(feature = "legacy-spec"),
    feature = "testing",
    feature = "server-macros",
    feature = "di"
))]

use neva::{App, di::Dc, testing::TestClient};

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

#[tokio::test]
async fn dc_extractor_is_injected_not_advertised() {
    let app = App::new().add_singleton(Counter { value: 41 });
    let client = TestClient::connect(app).await.expect("connect");

    // --- tools/list: assert the Dc dependency is not advertised ---
    let tools = client.tools().list(None).await.expect("tools/list").tools;
    let by_name = |name: &str| {
        let tool = tools
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("tool {name} not listed"));
        serde_json::to_value(tool).expect("tool json")
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
    let resp = client
        .tools()
        .call("read_counter", ())
        .await
        .expect("tools/call read_counter");
    let text = resp.content[0].as_text().map(|t| t.text.as_str());
    assert_eq!(
        text,
        Some("41"),
        "Dc<Counter> was not resolved from context"
    );

    // Call with the real arg too: dependency injected, arg deserialized.
    let resp = client
        .tools()
        .call("add_to_counter", [("delta", 1)])
        .await
        .expect("tools/call add_to_counter");
    let text = resp.content[0].as_text().map(|t| t.text.as_str());
    assert_eq!(
        text,
        Some("42"),
        "Dc<Counter> + real arg did not both resolve"
    );

    client.shutdown().await.expect("shutdown");
}

//! `x-mcp-header` header/body validation on the server (MCP 2026-07-28).
//!
//! A tool may ask the client to mirror an argument into `Mcp-Param-{name}` so
//! an intermediary can route or rate-limit on it without parsing the body.
//! That only holds if the origin server refuses to dispatch a call whose
//! headers say something the body does not -- which is what these exercise.
#![cfg(all(
    not(feature = "legacy-spec"),
    feature = "http-server-volga",
    feature = "http-client"
))]

use neva::App;

#[tokio::test(flavor = "multi_thread")]
async fn mirrored_headers_must_describe_the_call() {
    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app =
        App::new().with_options(|opt| opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp")));

    app.map_tool("query", |_region: String| async move { "ok".to_string() })
        .with_input_schema(|_| {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "region": { "type": "string", "x-mcp-header": "Region" }
                }
            })
            .into()
        });

    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("test client");
    let url = format!("http://{addr}/mcp");

    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "name": "query",
            "arguments": { "region": "us-west1" },
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }
    });

    let post = |extra: Vec<(&'static str, String)>| {
        let client = client.clone();
        let url = url.clone();
        let call = call.clone();
        async move {
            let mut req = client
                .post(&url)
                .header("MCP-Protocol-Version", "2026-07-28")
                .header("Mcp-Method", "tools/call")
                .header("Mcp-Name", "query");
            for (name, value) in extra {
                req = req.header(name, value);
            }
            req.json(&call).send().await.expect("send")
        }
    };

    // The honest call goes through.
    let resp = post(vec![("Mcp-Param-Region", "us-west1".into())]).await;
    assert!(resp.status().is_success(), "got {}", resp.status());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body.pointer("/result/content/0/text")
            .and_then(|v| v.as_str()),
        Some("ok"),
        "got: {body}"
    );

    // The header claims one region while the body carries another. This is the
    // whole point: an intermediary that let this through on `us-east1` must not
    // find the server running it against `us-west1`.
    let resp = post(vec![("Mcp-Param-Region", "us-east1".into())]).await;
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32020, "got: {body}");

    // The argument is present, so the client owed the header.
    let resp = post(vec![]).await;
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32020, "got: {body}");

    // An annotated argument the tool does not annotate is not the origin
    // server's business -- unrecognized `Mcp-Param-*` is forwarded and ignored.
    let resp = post(vec![
        ("Mcp-Param-Region", "us-west1".into()),
        ("Mcp-Param-Tenant", "acme".into()),
    ])
    .await;
    assert!(resp.status().is_success(), "got {}", resp.status());

    handle.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_header_without_the_argument_it_mirrors_is_rejected() {
    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app =
        App::new().with_options(|opt| opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp")));

    app.map_tool("query", || async move { "ok".to_string() })
        .with_input_schema(|_| {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "region": { "type": "string", "x-mcp-header": "Region" }
                }
            })
            .into()
        });

    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("test client");
    let url = format!("http://{addr}/mcp");

    // No `region` argument at all, but a header claiming one: nothing in the
    // body corresponds to what an intermediary was shown.
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "name": "query",
            "arguments": {},
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }
    });

    let resp = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "query")
        .header("Mcp-Param-Region", "us-west1")
        .json(&call)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32020, "got: {body}");

    handle.abort();
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

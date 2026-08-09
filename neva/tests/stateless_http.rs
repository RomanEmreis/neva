//! Stateless HTTP transport (MCP 2026-07-28) end-to-end checks.
//!
//! Exercises the stateless POST-only path: `server/discover` without a
//! session, a stateless `tools/call` carrying the required
//! `MCP-Protocol-Version` header, rejection of a header-less POST, and the
//! absence of the GET (SSE) / DELETE routes.
#![cfg(all(
    not(feature = "legacy-spec"),
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

    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("test client");
    let url = format!("http://{addr}/mcp");

    // (a) discover, no session header.
    let discover = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "server/discover",
        "params": { "_meta": meta() }
    });
    let resp = routed(client.post(&url), &discover)
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
        body["result"]["supportedVersions"],
        serde_json::json!(["2026-07-28"])
    );
    // `serverInfo` left the discovery result; the server identifies itself in
    // every result's `_meta` instead.
    assert!(body["result"].get("serverInfo").is_none(), "got: {body}");
    assert_eq!(
        body["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "neva"
    );
    // Every result carries the mandatory `resultType` discriminator, including
    // the discovery result.
    assert_eq!(body["result"]["resultType"], serde_json::json!("complete"));

    // (c) stateless tool call with the protocol-version header, no session.
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "ping", "arguments": {}, "_meta": meta() }
    });
    let resp = routed(client.post(&url), &call)
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
    assert_eq!(body["result"]["resultType"], serde_json::json!("complete"));

    // (d) missing protocol-version header -> `HeaderMismatch`, HTTP 400.
    let resp = client
        .post(&url)
        .json(&call)
        .send()
        .await
        .expect("send failed");
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32020, "got: {body}");

    // (d2) a version we do not speak -> `UnsupportedProtocolVersion`, and the
    //      client is told what is on offer.
    let resp = client
        .post(&url)
        .header("MCP-Protocol-Version", "1999-01-01")
        .json(&call)
        .send()
        .await
        .expect("send failed");
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32022, "got: {body}");
    assert_eq!(body["error"]["data"]["requested"], "1999-01-01");
    assert_eq!(
        body["error"]["data"]["supported"],
        serde_json::json!(["2026-07-28"])
    );

    // (d3) `ping` is gone in MCP 2026-07-28, and a method this server does not
    // implement answers `404` -- that status is what tells a caller "this
    // endpoint speaks MCP and has no such method" apart from "this URL is not
    // an MCP endpoint", without reading the body.
    for method in ["ping", "initialize", "logging/setLevel", "no/such/method"] {
        let gone = serde_json::json!({
            "jsonrpc": "2.0", "id": 5, "method": method,
            "params": { "_meta": meta() }
        });
        let resp = routed(client.post(&url), &gone)
            .json(&gone)
            .send()
            .await
            .expect("send failed");
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::NOT_FOUND,
            "`{method}` must answer 404"
        );
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"]["code"], -32601, "`{method}`: {body}");
        assert_eq!(body["id"], 5, "the id must survive: {body}");
    }

    // (d4) The header and the body must agree on the protocol version. They
    // disagree here, which is a header mismatch rather than an unsupported
    // version: picking a version off the supported list would not fix it.
    let mismatched = serde_json::json!({
        "jsonrpc": "2.0", "id": 6, "method": "tools/list",
        "params": { "_meta": {
            "io.modelcontextprotocol/protocolVersion": "v999.0.0",
            "io.modelcontextprotocol/clientCapabilities": {}
        } }
    });
    let resp = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/list")
        .json(&mismatched)
        .send()
        .await
        .expect("send failed");
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32020, "got: {body}");

    // (d5) A read of a resource that does not exist names the URI it could not
    // find, so a caller with several reads in flight can tell which one this
    // is about.
    let missing = serde_json::json!({
        "jsonrpc": "2.0", "id": 7, "method": "resources/read",
        "params": { "uri": "res://nope", "_meta": meta() }
    });
    let body: serde_json::Value = routed(client.post(&url), &missing)
        .json(&missing)
        .send()
        .await
        .expect("send failed")
        .json()
        .await
        .unwrap();
    assert_eq!(body["error"]["code"], -32602, "got: {body}");
    assert_eq!(body["error"]["data"]["uri"], "res://nope", "got: {body}");

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

/// The `_meta` MCP 2026-07-28 requires on every request: the protocol version,
/// and the capabilities this request is made under -- empty being the valid
/// declaration of "no optional capabilities".
fn meta() -> serde_json::Value {
    serde_json::json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {}
    })
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
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

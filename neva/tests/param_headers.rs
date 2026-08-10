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
        })
        .with_arg_names(["region"]);

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

    // The handler takes no argument at all, so there is nothing to name: the
    // annotated `region` property exists only to be claimed by a header.
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

/// A batch carries no routing headers at all -- one set cannot describe several
/// calls -- so the server must not demand the mirrored ones from a batched
/// call. Otherwise every batched call of an annotated tool would be rejected.
#[tokio::test(flavor = "multi_thread")]
async fn a_batched_call_of_an_annotated_tool_still_runs() {
    use neva::client::Client;

    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app =
        App::new().with_options(|opt| opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp")));

    app.map_tool("query", |region: String| async move { region })
        .with_input_schema(|_| {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "region": { "type": "string", "x-mcp-header": "Region" }
                }
            })
            .into()
        })
        .with_arg_names(["region"]);

    let handle = tokio::spawn(async move { app.run().await });

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match tokio::net::TcpStream::connect(&addr).await {
            Ok(_) => break,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await
            }
            Err(err) => panic!("server never became reachable: {err}"),
        }
    }

    let mut client = Client::new().with_options(|opt| {
        opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
            .with_timeout(std::time::Duration::from_secs(5))
    });
    client.connect().await.expect("connect");

    // The listing is what registers the annotation client-side.
    let tools = client.list_tools(None).await.expect("tools/list");
    assert_eq!(tools.tools.len(), 1, "the annotated tool must survive");

    let responses = client
        .batch()
        .call_tool("query", [("region", "us-west1")])
        .send()
        .await
        .expect("batch send");

    assert_eq!(responses.len(), 1);
    let result = responses
        .into_iter()
        .next()
        .expect("one response")
        .into_result::<serde_json::Value>()
        .expect("the batched call must not be rejected for missing headers");
    assert_eq!(
        result.pointer("/content/0/text").and_then(|v| v.as_str()),
        Some("us-west1"),
        "got: {result}"
    );

    handle.abort();
}

/// The whole SEP-2243 stale-schema loop, end to end against a real server.
///
/// neva's server states `ttlMs: 0` on `tools/list` -- the value the spec also
/// reads an absent `ttlMs` as -- so the listing a client just registered is
/// stale before it can be used. The call therefore goes out with no
/// `Mcp-Param-*`, the server refuses it with `HeaderMismatch`, the client
/// re-lists and retries with the headers, and the call succeeds. If any link
/// in that chain is missing, an annotated tool is simply uncallable.
#[tokio::test(flavor = "multi_thread")]
async fn an_annotated_tool_survives_a_listing_that_is_stale_on_arrival() {
    use neva::client::Client;

    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app =
        App::new().with_options(|opt| opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp")));

    app.map_tool("query", |region: String| async move { region })
        .with_input_schema(|_| {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "region": { "type": "string", "x-mcp-header": "Region" }
                }
            })
            .into()
        })
        .with_arg_names(["region"]);

    let handle = tokio::spawn(async move { app.run().await });

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match tokio::net::TcpStream::connect(&addr).await {
            Ok(_) => break,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await
            }
            Err(err) => panic!("server never became reachable: {err}"),
        }
    }

    let mut client = Client::new().with_options(|opt| {
        opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
            .with_timeout(std::time::Duration::from_secs(5))
    });
    client.connect().await.expect("connect");

    let tools = client.list_tools(None).await.expect("tools/list");
    assert_eq!(tools.ttl_ms, 0, "this test is about a zero-TTL listing");

    let result = client
        .call_tool("query", [("region", "us-west1")])
        .await
        .expect("the retry must carry the headers the first attempt omitted");

    assert_eq!(
        serde_json::to_value(&result)
            .ok()
            .as_ref()
            .and_then(|v| v.pointer("/content/0/text").and_then(|v| v.as_str())),
        Some("us-west1"),
        "got: {result:?}"
    );

    handle.abort();
}

/// The refusal recovery has to reach the tool it was sent back for, wherever
/// the server pages it.
///
/// A refreshed traversal starts over, clearing what the previous one
/// registered, so stopping at the first page would leave a later-paged tool
/// with no annotations at all -- and the retry would omit exactly the headers
/// it was refused for. The server pages at ten, so the annotated tool is named
/// to sort onto the second page.
#[tokio::test(flavor = "multi_thread")]
async fn the_refusal_recovery_pages_until_it_finds_the_tool() {
    use neva::client::Client;

    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app =
        App::new().with_options(|opt| opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp")));

    for i in 0..10 {
        app.map_tool(
            format!("a{i:02}_filler"),
            || async move { "ok".to_string() },
        );
    }

    app.map_tool("z_query", |region: String| async move { region })
        .with_input_schema(|_| {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "region": { "type": "string", "x-mcp-header": "Region" }
                }
            })
            .into()
        })
        .with_arg_names(["region"]);

    let handle = tokio::spawn(async move { app.run().await });

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match tokio::net::TcpStream::connect(&addr).await {
            Ok(_) => break,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await
            }
            Err(err) => panic!("server never became reachable: {err}"),
        }
    }

    let mut client = Client::new().with_options(|opt| {
        opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
            .with_timeout(std::time::Duration::from_secs(5))
    });
    client.connect().await.expect("connect");

    // Only the first page, which is exactly what leaves the annotated tool
    // unregistered.
    let page = client.list_tools(None).await.expect("tools/list");
    assert!(
        page.next_cursor.is_some() && !page.tools.iter().any(|t| &*t.name == "z_query"),
        "this test needs the annotated tool to sit past the first page"
    );

    let result = client
        .call_tool("z_query", [("region", "us-west1")])
        .await
        .expect("the recovery must page far enough to refresh the refused tool");

    assert_eq!(
        serde_json::to_value(&result)
            .ok()
            .as_ref()
            .and_then(|v| v.pointer("/content/0/text").and_then(|v| v.as_str())),
        Some("us-west1"),
        "got: {result:?}"
    );

    handle.abort();
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

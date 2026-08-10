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

/// A refresh that does not turn up the refused tool must leave the original
/// answer standing.
///
/// The refresh starts the traversal over and clears what the previous one
/// registered, so a tool the current listing no longer carries has nothing to
/// retry *with*: a second attempt goes out exactly as bare as the first, and
/// whatever it comes back with replaces the refusal that actually explained the
/// failure. A hand-rolled server here rather than an `App`, because the case is
/// a tool that answers a call while never appearing in `tools/list` -- which is
/// exactly what a real server does between withdrawing a tool and the caller
/// noticing.
#[tokio::test(flavor = "multi_thread")]
async fn a_tool_the_refresh_cannot_find_keeps_its_original_refusal() {
    use neva::client::Client;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let served = calls.clone();
    let handle = tokio::spawn(async move { serve_withdrawn_tool(listener, served).await });

    let mut client = Client::new().with_options(|opt| {
        opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
            .with_timeout(std::time::Duration::from_secs(5))
    });
    client.connect().await.expect("connect");

    let err = client
        .call_tool("withdrawn", [("region", "us-west1")])
        .await
        .expect_err("a call the server refuses for missing headers stays refused");

    assert!(
        err.to_string().contains("Missing Mcp-Param-Region"),
        "the refusal that explains the failure must survive the refresh, got: {err}"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "with nothing refreshed there is nothing to retry with, so no second call"
    );

    handle.abort();
}

/// Answers discovery, an empty `tools/list`, and refuses every `tools/call`
/// the way a server refuses a call missing its mirrored headers.
async fn serve_withdrawn_tool(
    listener: tokio::net::TcpListener,
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    use tokio::io::AsyncReadExt;

    while let Ok((mut stream, _)) = listener.accept().await {
        let mut buf = vec![0u8; 8192];
        let read = match stream.read(&mut buf).await {
            Ok(0) | Err(_) => continue,
            Ok(n) => n,
        };
        let request = String::from_utf8_lossy(&buf[..read]).to_string();
        let body = request.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        let id = parsed.get("id").cloned().unwrap_or(serde_json::Value::Null);

        let result = match parsed.get("method").and_then(|m| m.as_str()) {
            Some("server/discover") => serde_json::json!({
                "supportedVersions": ["2026-07-28"],
                "capabilities": { "tools": {} },
                "ttlMs": 0,
                "cacheScope": "private"
            }),
            // Never lists the tool, so no refresh can register it.
            Some("tools/list") => serde_json::json!({
                "tools": [], "ttlMs": 0, "cacheScope": "private"
            }),
            Some("tools/call") => {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let refusal = serde_json::json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": {
                        "code": -32020,
                        "message": "Missing Mcp-Param-Region header for the mirrored argument"
                    }
                });
                write_json(&mut stream, &refusal).await;
                continue;
            }
            _ => serde_json::json!({}),
        };

        let reply = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
        write_json(&mut stream, &reply).await;
    }
}

async fn write_json(stream: &mut tokio::net::TcpStream, body: &serde_json::Value) {
    use tokio::io::AsyncWriteExt;

    let body = body.to_string();
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.flush().await;
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

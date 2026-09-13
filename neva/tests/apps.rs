//! MCP Apps negotiation over the stateless 2026-07-28 transport.
//!
//! There is no handshake under 2026-07-28, so a client declares
//! `io.modelcontextprotocol/ui` on every request's `_meta`, and a handler reads
//! it back with `Context::supports_apps` to shape its answer for the caller.
//! These join the two halves: what a neva client writes is what a neva server
//! reads.
#![cfg(all(
    not(feature = "legacy-spec"),
    feature = "apps",
    feature = "http-server-volga",
    feature = "http-client"
))]

use neva::{App, Context, client::Client};

/// A UI-bound tool that answers with bare data when a UI will present it, and
/// with a sentence when the text is all the caller gets.
async fn serve(addr: &str) -> tokio::task::JoinHandle<()> {
    let mut app = App::new().with_options(|opt| {
        opt.with_http(|http| http.bind(addr).with_endpoint("/mcp"))
            .with_apps()
    });

    app.map_tool("get_time", |ctx: Context| async move {
        let now = "12:00:00 UTC";
        if ctx.supports_apps() {
            now.to_string()
        } else {
            format!("The time is {now}.")
        }
    });

    app.map_tool("search_mode", |ctx: Context| async move {
        ctx.client_extension("com.example/search")
            .map_or_else(|| "none".to_string(), |settings| settings.to_string())
    });

    let handle = tokio::spawn(async move { app.run().await });

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match tokio::net::TcpStream::connect(addr).await {
            Ok(_) => break,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await
            }
            Err(err) => panic!("server never became reachable: {err}"),
        }
    }

    handle
}

async fn get_time(client: &mut Client) -> String {
    let resp = client.call_tool("get_time", ()).await.expect("tools/call");
    resp.content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .expect("a text result")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_declaring_apps_is_seen_as_one() {
    let addr = format!("127.0.0.1:{}", pick_free_port());
    let handle = serve(&addr).await;

    let mut client = Client::new().with_options(|opt| {
        opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
            .with_timeout(std::time::Duration::from_secs(5))
            .with_apps()
    });
    client.connect().await.expect("connect");

    assert_eq!(get_time(&mut client).await, "12:00:00 UTC");

    client.disconnect().await.ok();
    handle.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_declaring_nothing_gets_the_text_fallback() {
    let addr = format!("127.0.0.1:{}", pick_free_port());
    let handle = serve(&addr).await;

    let mut client = Client::new().with_options(|opt| {
        opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
            .with_timeout(std::time::Duration::from_secs(5))
    });
    client.connect().await.expect("connect");

    assert_eq!(get_time(&mut client).await, "The time is 12:00:00 UTC.");

    client.disconnect().await.ok();
    handle.abort();
}

/// What a peer other than neva may send: the extension key with no
/// `mimeTypes`, which the specification makes required, and an extension neva
/// knows nothing about, which a handler can still read.
#[tokio::test(flavor = "multi_thread")]
async fn the_server_reads_the_map_off_the_wire() {
    let addr = format!("127.0.0.1:{}", pick_free_port());
    let handle = serve(&addr).await;
    let http = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("test client");
    let url = format!("http://{addr}/mcp");

    let call = |id: u64, name: &str, extensions: serde_json::Value| {
        serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": name, "arguments": {},
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": { "extensions": extensions }
                } }
        })
    };
    let text = |resp: serde_json::Value| {
        resp.pointer("/result/content/0/text")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| panic!("a text result: {resp}"))
    };

    for (id, extensions, expected) in [
        (
            1,
            serde_json::json!({ "io.modelcontextprotocol/ui": {} }),
            "The time is 12:00:00 UTC.",
        ),
        (
            2,
            serde_json::json!({
                "io.modelcontextprotocol/ui": { "mimeTypes": ["text/html;profile=mcp-app"] }
            }),
            "12:00:00 UTC",
        ),
    ] {
        let body = call(id, "get_time", extensions);
        let resp = routed(http.post(&url), &body)
            .json(&body)
            .send()
            .await
            .expect("send")
            .json()
            .await
            .expect("json");
        assert_eq!(text(resp), expected, "request {id}");
    }

    let body = call(
        3,
        "search_mode",
        serde_json::json!({ "com.example/search": { "fuzzy": true } }),
    );
    let resp = routed(http.post(&url), &body)
        .json(&body)
        .send()
        .await
        .expect("send")
        .json()
        .await
        .expect("json");
    assert_eq!(text(resp), r#"{"fuzzy":true}"#);

    handle.abort();
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// Attaches the routing headers MCP 2026-07-28 requires on every request.
fn routed(req: reqwest::RequestBuilder, body: &serde_json::Value) -> reqwest::RequestBuilder {
    let req = req
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call");
    match body.pointer("/params/name").and_then(|v| v.as_str()) {
        Some(name) => req.header("Mcp-Name", name),
        None => req,
    }
}

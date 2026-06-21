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

use neva::{
    App, Context,
    client::Client,
    error::Error,
    types::elicitation::{ElicitRequestParams, ElicitResult},
};

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

use std::sync::atomic::{AtomicUsize, Ordering};

static FETCHES: AtomicUsize = AtomicUsize::new(0);
static CHARGES: AtomicUsize = AtomicUsize::new(0);
static RECEIPTS: AtomicUsize = AtomicUsize::new(0);
static LOST_RESPONSE_COMMITS: AtomicUsize = AtomicUsize::new(0);
static CONCURRENT_FINAL_COMMITS: AtomicUsize = AtomicUsize::new(0);

#[tokio::test(flavor = "multi_thread")]
async fn final_round_replay_is_idempotent_after_a_lost_response() {
    // The final POST commits and produces a result, but its HTTP response is
    // "lost"; the client retries the SAME requestState + inputResponses. The
    // server must serve the cached result without re-running the handler — so
    // the on_commit side effect fires exactly once across both finals.
    LOST_RESPONSE_COMMITS.store(0, Ordering::SeqCst);

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
        ctx.on_commit(async move {
            LOST_RESPONSE_COMMITS.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
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

    // The final retry, reused verbatim for both the "lost" send and the replay.
    let retry = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "greet", "arguments": {},
            "_meta": {
                "clientCapabilities": { "elicitation": true },
                "requestState": state,
                "inputResponses": { key: { "action": "accept", "content": { "name": "octocat" } } }
            } }
    });

    // Round 2 (final): completes and runs the commit. Pretend the response is lost.
    let r2: serde_json::Value = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&retry)
        .send()
        .await
        .expect("final send")
        .json()
        .await
        .expect("final json");
    assert_eq!(
        r2.pointer("/result/content/0/text")
            .and_then(|v| v.as_str()),
        Some("hello octocat"),
        "final round must complete: {r2}"
    );
    assert_eq!(LOST_RESPONSE_COMMITS.load(Ordering::SeqCst), 1);

    // Lost-response retry: identical requestState + inputResponses, new id.
    let mut replay = retry.clone();
    replay["id"] = serde_json::json!(3);
    let r3: serde_json::Value = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&replay)
        .send()
        .await
        .expect("replay send")
        .json()
        .await
        .expect("replay json");

    // Same result, the retry's own id, and the commit did NOT fire again.
    assert_eq!(
        r3.pointer("/result/content/0/text")
            .and_then(|v| v.as_str()),
        Some("hello octocat"),
        "replay must return the cached result: {r3}"
    );
    assert_eq!(
        r3["id"],
        serde_json::json!(3),
        "cached response adopts the retry id"
    );
    assert_eq!(
        LOST_RESPONSE_COMMITS.load(Ordering::SeqCst),
        1,
        "on_commit must not fire again on a lost-response retry"
    );

    handle.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_final_round_retries_commit_exactly_once() {
    // Two IDENTICAL final-round retries arrive at the same time (the client
    // timed out and re-sent while the first is still executing). Without a
    // per-state reservation both miss the idempotency cache and re-run the
    // handler + on_commit. The handler sleeps to widen that window; the
    // reservation must still serialise them so the commit fires exactly once.
    CONCURRENT_FINAL_COMMITS.store(0, Ordering::SeqCst);

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
        // Widen the get-miss → put window so both retries would overlap absent
        // the reservation.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        ctx.on_commit(async move {
            CONCURRENT_FINAL_COMMITS.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
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

    // Two identical final retries, distinct ids, fired concurrently.
    let retry = |id: i64| {
        serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": "greet", "arguments": {},
                "_meta": {
                    "clientCapabilities": { "elicitation": true },
                    "requestState": state,
                    "inputResponses": { key.clone(): { "action": "accept", "content": { "name": "octocat" } } }
                } }
        })
    };
    let send = |body: serde_json::Value| {
        let client = client.clone();
        let url = url.clone();
        async move {
            client
                .post(&url)
                .header("MCP-Protocol-Version", "2026-07-28")
                .json(&body)
                .send()
                .await
                .expect("final send")
                .json::<serde_json::Value>()
                .await
                .expect("final json")
        }
    };

    let (ra, rb) = tokio::join!(send(retry(2)), send(retry(3)));

    // Both retries succeed with the same result; the commit fired only once.
    for r in [&ra, &rb] {
        assert_eq!(
            r.pointer("/result/content/0/text").and_then(|v| v.as_str()),
            Some("hello octocat"),
            "both concurrent finals must return the result: {r}"
        );
    }
    assert_eq!(
        CONCURRENT_FINAL_COMMITS.load(Ordering::SeqCst),
        1,
        "on_commit must fire exactly once across concurrent identical retries"
    );

    handle.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn distinct_answers_to_the_same_state_do_not_collide_in_the_cache() {
    // Two flows reach the SAME pre-answer requestState (same method/params/
    // principal, no nonce) but supply DIFFERENT inputResponses. The final cache
    // is keyed by the state tag plus the answers digest, so the second flow must
    // see its own answer reflected — never the first flow's cached result.
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

    // Two finals share the same state but answer with different names.
    let final_with = |id: i64, name: &str| {
        serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": "greet", "arguments": {},
                "_meta": {
                    "clientCapabilities": { "elicitation": true },
                    "requestState": state,
                    "inputResponses": { key.clone(): { "action": "accept", "content": { "name": name } } }
                } }
        })
    };

    let post = |body: serde_json::Value| {
        let client = client.clone();
        let url = url.clone();
        async move {
            client
                .post(&url)
                .header("MCP-Protocol-Version", "2026-07-28")
                .json(&body)
                .send()
                .await
                .expect("send")
                .json::<serde_json::Value>()
                .await
                .expect("json")
        }
    };

    let r_a = post(final_with(2, "octocat")).await;
    let r_b = post(final_with(3, "monalisa")).await;

    assert_eq!(
        r_a.pointer("/result/content/0/text")
            .and_then(|v| v.as_str()),
        Some("hello octocat"),
        "first flow gets its own answer: {r_a}"
    );
    assert_eq!(
        r_b.pointer("/result/content/0/text")
            .and_then(|v| v.as_str()),
        Some("hello monalisa"),
        "second flow must NOT receive the first flow's cached result: {r_b}"
    );

    handle.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn effects_run_once_memo_caches_commit_fires_on_final_round() {
    FETCHES.store(0, Ordering::SeqCst);
    CHARGES.store(0, Ordering::SeqCst);
    RECEIPTS.store(0, Ordering::SeqCst);

    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app = App::new()
        .with_request_state_secret(b"test-secret")
        .with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")));

    app.map_tool("effectful", |mut ctx: Context| async move {
        let price: i32 = ctx
            .memo("quote", async {
                FETCHES.fetch_add(1, Ordering::SeqCst);
                Ok(42)
            })
            .await?;
        ctx.once("charge", async {
            CHARGES.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await?;
        ctx.on_commit(async {
            RECEIPTS.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        let params: ElicitRequestParams = ElicitRequestParams::form("Your name?")
            .with_required("name", "string")
            .into();
        let res = ctx.elicit("name", params).await?;
        let name = res
            .content
            .and_then(|c| c.get("name").and_then(|v| v.as_str().map(str::to_owned)))
            .unwrap_or_else(|| "stranger".into());
        Ok::<String, Error>(format!("hello {name}, charged at {price}"))
    });

    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/mcp");

    // Round 1: input_required. Effect + memo ran; commit NOT yet.
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "effectful", "arguments": {},
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
    assert_eq!(
        FETCHES.load(Ordering::SeqCst),
        1,
        "memo computed in round 1"
    );
    assert_eq!(CHARGES.load(Ordering::SeqCst), 1, "once ran in round 1");
    assert_eq!(
        RECEIPTS.load(Ordering::SeqCst),
        0,
        "commit must not fire yet"
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

    // Round 2: retry → final result. memo HIT (no fetch), once HIT (no charge),
    // commit fires exactly once.
    let retry = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "effectful", "arguments": {},
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
        Some("hello octocat, charged at 42"),
        "round 2 must complete with memoized price: {r2}"
    );
    assert_eq!(
        FETCHES.load(Ordering::SeqCst),
        1,
        "memo not recomputed on round 2"
    );
    assert_eq!(
        CHARGES.load(Ordering::SeqCst),
        1,
        "once not re-run on round 2"
    );
    assert_eq!(
        RECEIPTS.load(Ordering::SeqCst),
        1,
        "commit fired exactly once on final round"
    );

    handle.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_request_state_is_rejected() {
    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app = App::new()
        .with_request_state_secret(b"test-secret")
        .with_max_state_bytes(256) // smaller than the memoized payload
        .with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")));

    app.map_tool("bloated", |mut ctx: Context| async move {
        let big: String = ctx.memo("big", async { Ok("x".repeat(2048)) }).await?;
        let params: ElicitRequestParams = ElicitRequestParams::form("Your name?")
            .with_required("name", "string")
            .into();
        let _ = ctx.elicit("name", params).await?;
        Ok::<String, Error>(big)
    });

    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/mcp");

    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "bloated", "arguments": {},
            "_meta": { "clientCapabilities": { "elicitation": true } } }
    });
    let r1: serde_json::Value = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&call)
        .send()
        .await
        .expect("send")
        .json()
        .await
        .expect("json");
    let msg = r1
        .pointer("/error/message")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        msg.contains("requestState too large"),
        "oversized state must be rejected: {r1}"
    );

    handle.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_inbound_request_state_is_rejected_before_decoding() {
    // An untrusted client supplies a bogus `requestState` far larger than the
    // configured cap. It must be rejected on size *before* base64 decoding and
    // HMAC verification run, so the cap protects inbound retries — not just the
    // outbound states the server mints.
    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app = App::new()
        .with_request_state_secret(b"test-secret")
        .with_max_state_bytes(256)
        .with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")));

    app.map_tool("greet", |mut ctx: Context| async move {
        let params: ElicitRequestParams = ElicitRequestParams::form("Your name?")
            .with_required("name", "string")
            .into();
        let _ = ctx.elicit("name", params).await?;
        Ok::<String, Error>("ok".into())
    });

    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/mcp");

    // A 4 KiB blob — well over the 256-byte cap and never a valid signed state.
    let bogus_state = "A".repeat(4096);
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "greet", "arguments": {},
            "_meta": {
                "clientCapabilities": { "elicitation": true },
                "requestState": bogus_state
            } }
    });
    let r1: serde_json::Value = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&call)
        .send()
        .await
        .expect("send")
        .json()
        .await
        .expect("json");
    let msg = r1
        .pointer("/error/message")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        msg.contains("exceeds the configured maximum size"),
        "oversized inbound state must be rejected before decoding: {r1}"
    );

    handle.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn replaying_request_state_against_a_different_request_is_rejected() {
    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app = App::new()
        .with_request_state_secret(b"test-secret")
        .with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")));

    app.map_tool("greet", |mut ctx: Context| async move {
        let params: ElicitRequestParams = ElicitRequestParams::form("Your name?")
            .with_required("name", "string")
            .into();
        let _ = ctx.elicit("name", params).await?;
        Ok::<String, Error>("ok".into())
    });

    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/mcp");

    // Round 1: bind state to `arguments: {}`.
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
    let state = r1["result"]["requestState"]
        .as_str()
        .expect("requestState present")
        .to_string();

    // Replay that state against a request with DIFFERENT arguments → the
    // request binding no longer matches.
    let replay = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "greet", "arguments": { "x": 1 },
            "_meta": {
                "clientCapabilities": { "elicitation": true },
                "requestState": state
            } }
    });
    let r2: serde_json::Value = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&replay)
        .send()
        .await
        .expect("replay send")
        .json()
        .await
        .expect("replay json");
    let msg = r2
        .pointer("/error/message")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        msg.contains("does not match this request"),
        "replayed state must be bound to the original request: {r2}"
    );

    handle.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn eliciting_without_declared_capability_is_rejected() {
    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app = App::new()
        .with_request_state_secret(b"test-secret")
        .with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")));

    app.map_tool("greet", |mut ctx: Context| async move {
        let params: ElicitRequestParams = ElicitRequestParams::form("Your name?")
            .with_required("name", "string")
            .into();
        let _ = ctx.elicit("name", params).await?;
        Ok::<String, Error>("ok".into())
    });

    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/mcp");

    // No `clientCapabilities.elicitation` → the server cannot ask for input.
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "greet", "arguments": {} }
    });
    let r1: serde_json::Value = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&call)
        .send()
        .await
        .expect("send")
        .json()
        .await
        .expect("json");
    let msg = r1
        .pointer("/error/message")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        msg.contains("did not declare support"),
        "elicitation without declared capability must be rejected: {r1}"
    );

    handle.abort();
}

// Separate counters from the reqwest `effectful` tool so the two tests can run
// in parallel without racing on shared process-global state.
static C_FETCHES: AtomicUsize = AtomicUsize::new(0);
static C_CHARGES: AtomicUsize = AtomicUsize::new(0);
static C_RECEIPTS: AtomicUsize = AtomicUsize::new(0);

/// Real end-to-end: the neva MCP **client** (not raw reqwest) drives the whole
/// MRTR loop — `connect()` runs `server/discover`, `call_tool` enters
/// `run_with_mrtr`, and the registered elicitation handler answers the
/// server's request transparently across the round-trip.
#[tokio::test(flavor = "multi_thread")]
async fn client_drives_mrtr_elicitation_end_to_end() {
    C_FETCHES.store(0, Ordering::SeqCst);
    C_CHARGES.store(0, Ordering::SeqCst);
    C_RECEIPTS.store(0, Ordering::SeqCst);

    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app = App::new()
        .with_request_state_secret(b"test-secret")
        .with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")));

    app.map_tool("client_effectful", |mut ctx: Context| async move {
        let price: i32 = ctx
            .memo("quote", async {
                C_FETCHES.fetch_add(1, Ordering::SeqCst);
                Ok(42)
            })
            .await?;
        ctx.once("charge", async {
            C_CHARGES.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await?;
        ctx.on_commit(async {
            C_RECEIPTS.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        let params: ElicitRequestParams = ElicitRequestParams::form("Your name?")
            .with_required("name", "string")
            .into();
        let res = ctx.elicit("name", params).await?;
        let name = res
            .content
            .and_then(|c| c.get("name").and_then(|v| v.as_str().map(str::to_owned)))
            .unwrap_or_else(|| "stranger".into());
        Ok::<String, Error>(format!("hello {name}, charged at {price}"))
    });

    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // The client declares `clientCapabilities.elicitation` automatically because
    // an elicitation handler is registered; the handler answers every prompt.
    let mut client =
        Client::new().with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")));
    client.map_elicitation(|_params: ElicitRequestParams| async move {
        ElicitResult::accept().with_content(serde_json::json!({ "name": "octocat" }))
    });
    client.connect().await.expect("client connects");

    let resp = client
        .call_tool("client_effectful", ())
        .await
        .expect("tool call completes through the MRTR loop");

    let text = resp
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.as_str());
    assert_eq!(
        text,
        Some("hello octocat, charged at 42"),
        "client should receive the final, memoized result"
    );
    assert!(!resp.is_error, "final result must not be an error");

    // The whole loop ran once front-to-back: effect once, memo once, commit once.
    assert_eq!(C_FETCHES.load(Ordering::SeqCst), 1, "memo computed once");
    assert_eq!(C_CHARGES.load(Ordering::SeqCst), 1, "once ran once");
    assert_eq!(C_RECEIPTS.load(Ordering::SeqCst), 1, "commit fired once");

    client.disconnect().await.ok();
    handle.abort();
}

/// A batch whose requests elicit must be driven through the MRTR loop just like
/// single sends: each eliciting `tools/call` is fulfilled and re-issued (with
/// `inputResponses` + the echoed `requestState`) until it produces a final
/// result, never leaving the protocol-intermediate `input_required` as the
/// batch's answer. Non-eliciting requests and notifications ride the same batch,
/// keep their slots in order, and notifications produce no slot.
#[tokio::test(flavor = "multi_thread")]
async fn client_drives_mrtr_across_a_batch_end_to_end() {
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

    let mut client =
        Client::new().with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")));
    client.map_elicitation(|_params: ElicitRequestParams| async move {
        ElicitResult::accept().with_content(serde_json::json!({ "name": "octocat" }))
    });
    client.connect().await.expect("client connects");

    // A mixed batch: a non-eliciting list, two eliciting tool calls, and a
    // fire-and-forget notification interleaved between them.
    let responses = client
        .batch()
        .list_tools()
        .call_tool("greet", ())
        .notify("notifications/progress", None)
        .call_tool("greet", ())
        .send()
        .await
        .expect("batch completes through the MRTR loop");

    // Three slots (the notification produces none), in request order.
    assert_eq!(
        responses.len(),
        3,
        "one slot per request, notifications none"
    );

    // Slot 0: the non-eliciting tools/list result.
    let tools = responses[0]
        .clone()
        .into_result::<neva::types::ListToolsResult>()
        .expect("tools/list result");
    assert!(
        tools.tools.iter().any(|t| t.name == "greet"),
        "first slot is the tools/list result"
    );

    // Slots 1 & 2: both eliciting calls were driven to their final results,
    // never returning `input_required`.
    for (slot, resp) in [(1usize, &responses[1]), (2, &responses[2])] {
        let result = resp
            .clone()
            .into_result::<neva::types::CallToolResponse>()
            .unwrap_or_else(|e| panic!("slot {slot} is a final tools/call result: {e}"));
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str());
        assert_eq!(
            text,
            Some("hello octocat"),
            "slot {slot} must carry the elicited final result"
        );
        assert!(!result.is_error, "slot {slot} must not be an error");
    }

    client.disconnect().await.ok();
    handle.abort();
}

/// The MRTR round cap is configurable via `with_max_mrtr_rounds`. With a cap of
/// 1, a tool that elicits never converges within the budget, so the client gives
/// up with the max-rounds error instead of looping the default 8 times.
#[tokio::test(flavor = "multi_thread")]
async fn configurable_max_rounds_caps_the_mrtr_loop() {
    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app = App::new()
        .with_request_state_secret(b"test-secret")
        .with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")));

    app.map_tool("greet", |mut ctx: Context| async move {
        let params: ElicitRequestParams = ElicitRequestParams::form("Your name?")
            .with_required("name", "string")
            .into();
        let _ = ctx.elicit("name", params).await?;
        Ok::<String, Error>("done".into())
    });

    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let mut client = Client::new().with_options(|o| {
        o.with_http(|h| h.bind(&addr).with_endpoint("/mcp"))
            .with_max_mrtr_rounds(1)
    });
    client.map_elicitation(|_params: ElicitRequestParams| async move {
        ElicitResult::accept().with_content(serde_json::json!({ "name": "octocat" }))
    });
    client.connect().await.expect("client connects");

    let err = client
        .call_tool("greet", ())
        .await
        .expect_err("a 1-round cap must not let the elicitation converge");
    assert!(
        err.to_string().contains("maximum number of rounds"),
        "expected the max-rounds error, got: {err}"
    );

    client.disconnect().await.ok();
    handle.abort();
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

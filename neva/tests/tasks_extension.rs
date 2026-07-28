//! Tasks-as-extension (MCP 2026-07-28) end-to-end checks.
//!
//! Under MCP 2026-07-28 the tasks capability is advertised through the extensions
//! map (`capabilities.extensions["io.modelcontextprotocol/tasks"]`) instead of
//! the former top-level `capabilities.tasks` field, and the method surface is
//! `tasks/get` (polling, carrying the terminal result inline), `tasks/update`
//! (answering input requests) and `tasks/cancel` -- with `tasks/list` and
//! `tasks/result` gone. Exercised over the stateless POST-only path.
#![cfg(all(
    not(feature = "legacy-spec"),
    feature = "tasks",
    feature = "http-server-volga",
    feature = "http-client"
))]

use neva::{App, Context, error::Error, types::elicitation::ElicitRequestParams};
use std::sync::atomic::{AtomicUsize, Ordering};

static TASK_COMMITS: AtomicUsize = AtomicUsize::new(0);

#[tokio::test(flavor = "multi_thread")]
async fn tasks_capability_is_advertised_as_extension() {
    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app = App::new().with_options(|opt| {
        opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
            .with_tasks()
    });
    app.map_tool("ping", || async move { "pong".to_string() });
    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("test client");
    let url = format!("http://{addr}/mcp");

    // (a) discover advertises tasks under the extensions map, not top-level.
    let discover = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": {}
    });
    let resp = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&discover)
        .send()
        .await
        .expect("discover failed");
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    let caps = &body["result"]["capabilities"];

    assert!(
        caps["extensions"]["io.modelcontextprotocol/tasks"].is_object(),
        "tasks must be advertised under capabilities.extensions, got: {caps}"
    );
    assert!(
        caps.get("tasks").is_none(),
        "no top-level capabilities.tasks under MCP 2026-07-28, got: {caps}"
    );

    // (b) the removed methods no longer dispatch.
    for gone in ["tasks/list", "tasks/result"] {
        let req = serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": gone, "params": {}
        });
        let resp = client
            .post(&url)
            .header("MCP-Protocol-Version", "2026-07-28")
            .json(&req)
            .send()
            .await
            .expect("send failed");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(
            body["error"]["code"], -32601,
            "{gone} must be gone under MCP 2026-07-28, got: {body}"
        );
    }

    // (c) `tasks/update` is routable (an unknown task is a params error, not an
    // unknown method).
    let update = serde_json::json!({
        "jsonrpc": "2.0", "id": 3, "method": "tasks/update",
        "params": { "taskId": "nope", "inputResponses": {} }
    });
    let resp = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&update)
        .send()
        .await
        .expect("send failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["error"]["code"], -32602,
        "tasks/update must dispatch, got: {body}"
    );

    handle.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn task_augmented_tool_elicits_via_suspend_resume() {
    // A task-augmented tool that elicits runs on the *stateful* task substrate,
    // not MRTR: it uses the explicit `ctx.task().elicit(...)` builder, the
    // background future suspends (task -> input_required), the client posts the
    // answer as a Response keyed by the task id (session-independent), the future
    // resumes in place, and the final result carries the elicited value. Side
    // effects are just run inline (no MRTR `on_commit` needed -- there is no
    // re-run); the counter below proves the resumed body ran to completion.
    TASK_COMMITS.store(0, Ordering::SeqCst);

    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app = App::new().with_options(|opt| {
        opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
            .with_tasks()
    });
    app.map_tool("greet_task", |mut ctx: Context| async move {
        let params: ElicitRequestParams = ElicitRequestParams::form("Your name?")
            .with_required("name", "string")
            .into();
        let res = ctx.task().elicit(params).await?;
        let name = res
            .content
            .and_then(|c| c.get("name").and_then(|v| v.as_str().map(str::to_owned)))
            .unwrap_or_else(|| "stranger".into());
        // Inline side effect after the elicit resumes (the task runs once).
        TASK_COMMITS.fetch_add(1, Ordering::SeqCst);
        Ok::<String, Error>(format!("hello {name}"))
    })
    .with_task_support("optional");

    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("test client");
    let url = format!("http://{addr}/mcp");

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

    let wait_status = |target: &'static str, task_id: String| {
        let post = &post;
        async move {
            for _ in 0..100 {
                let g = post(serde_json::json!({
                    "jsonrpc": "2.0", "id": 2, "method": "tasks/get",
                    "params": { "taskId": task_id }
                }))
                .await;
                if g["result"]["status"].as_str() == Some(target) {
                    return true;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            false
        }
    };

    // 1. Task-augmented call -> CreateTaskResult carrying a task id.
    let r1 = post(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "name": "greet_task", "arguments": {},
            "task": { "ttl": 60000 },
            "_meta": { "io.modelcontextprotocol/clientCapabilities": { "elicitation": true } }
        }
    }))
    .await;
    assert_eq!(
        r1["result"]["resultType"], "task",
        "a deferred result is tagged `task`, got: {r1}"
    );
    let task_id = r1["result"]["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("task id present, got: {r1}"))
        .to_string();

    // 2. The tool elicits -> the task suspends into input_required.
    assert!(
        wait_status("input_required", task_id.clone()).await,
        "task must enter input_required when the tool elicits"
    );

    // 3. `tasks/get` surfaces the outstanding ask; answer it with `tasks/update`
    //    under the same key.
    let g = post(serde_json::json!({
        "jsonrpc": "2.0", "id": 3, "method": "tasks/get",
        "params": { "taskId": task_id }
    }))
    .await;
    let key = g["result"]["inputRequests"]
        .as_object()
        .unwrap_or_else(|| panic!("inputRequests present, got: {g}"))
        .keys()
        .next()
        .expect("one outstanding ask")
        .clone();
    assert_eq!(
        g["result"]["inputRequests"][&key]["method"], "elicitation/create",
        "the ask is surfaced as a {{method, params}} envelope, got: {g}"
    );

    post(serde_json::json!({
        "jsonrpc": "2.0", "id": 4, "method": "tasks/update",
        "params": {
            "taskId": task_id,
            "inputResponses": {
                key: { "action": "accept", "content": { "name": "octocat" } }
            }
        }
    }))
    .await;

    // 4. The future resumes and runs to completion.
    assert!(
        wait_status("completed", task_id.clone()).await,
        "task must complete after the answer is delivered"
    );

    // 5. `tasks/get` carries the final result inline -- there is no
    //    `tasks/result` to follow up with.
    let r = post(serde_json::json!({
        "jsonrpc": "2.0", "id": 99, "method": "tasks/get",
        "params": { "taskId": task_id }
    }))
    .await;
    assert_eq!(
        r.pointer("/result/result/content/0/text")
            .and_then(|v| v.as_str()),
        Some("hello octocat"),
        "a completed task carries its result inline, got: {r}"
    );
    assert_eq!(
        r["result"]["resultType"], "complete",
        "the `tasks/get` result itself is complete, got: {r}"
    );
    assert_eq!(
        TASK_COMMITS.load(Ordering::SeqCst),
        1,
        "the resumed task body must run to completion exactly once"
    );

    handle.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn mrtr_elicit_inside_a_task_is_rejected_with_guidance() {
    // The MRTR `ctx.elicit` is not valid on the task substrate -- it must guide
    // the author to `ctx.task().elicit(...)` rather than silently misbehave.
    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app = App::new().with_options(|opt| {
        opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
            .with_tasks()
    });
    app.map_tool("bad_elicit", |mut ctx: Context| async move {
        let params: ElicitRequestParams = ElicitRequestParams::form("Your name?")
            .with_required("name", "string")
            .into();
        // Wrong API for a task: this must error, not suspend.
        let res = ctx.elicit("name", params).await?;
        Ok::<String, Error>(format!("{:?}", res.content))
    })
    .with_task_support("required");

    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("test client");
    let url = format!("http://{addr}/mcp");
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

    let r1 = post(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "name": "bad_elicit", "arguments": {},
            "task": { "ttl": 60000 },
            "_meta": { "io.modelcontextprotocol/clientCapabilities": { "elicitation": true } }
        }
    }))
    .await;
    assert_eq!(
        r1["result"]["resultType"], "task",
        "a deferred result is tagged `task`, got: {r1}"
    );
    let task_id = r1["result"]["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("task id present, got: {r1}"))
        .to_string();

    // The tool errors immediately; poll for a terminal state then read the error.
    let mut text = String::new();
    for _ in 0..100 {
        let r = post(serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "tasks/get",
            "params": { "taskId": task_id }
        }))
        .await;
        // A *tool* error is a successful `tools/call` result carrying
        // `isError`, not a JSON-RPC failure -- so it rides in the task's
        // `result`, and the task itself completes. (`error` is reserved for a
        // protocol-level failure during execution.)
        if let Some(t) = r
            .pointer("/result/result/content/0/text")
            .and_then(|v| v.as_str())
        {
            text = t.to_string();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        text.contains("ctx.task().elicit"),
        "MRTR elicit in a task must guide to ctx.task().elicit, got: {text:?}"
    );

    handle.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn mrtr_once_in_a_required_task_is_rejected() {
    // `once` is an MRTR helper; in a required-task tool (which never re-runs) it
    // must error rather than silently masquerade as a dedup.
    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app = App::new().with_options(|opt| {
        opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
            .with_tasks()
    });
    app.map_tool("bad_once", |ctx: Context| async move {
        ctx.once("x", async { Ok(()) }).await?;
        Ok::<String, Error>("unreachable".into())
    })
    .with_task_support("required");

    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("test client");
    let url = format!("http://{addr}/mcp");
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

    let r1 = post(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "bad_once", "arguments": {}, "task": { "ttl": 60000 } }
    }))
    .await;
    assert_eq!(
        r1["result"]["resultType"], "task",
        "a deferred result is tagged `task`, got: {r1}"
    );
    let task_id = r1["result"]["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("task id present, got: {r1}"))
        .to_string();

    let mut text = String::new();
    for _ in 0..100 {
        let r = post(serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "tasks/get",
            "params": { "taskId": task_id }
        }))
        .await;
        // A *tool* error is a successful `tools/call` result carrying
        // `isError`, not a JSON-RPC failure -- so it rides in the task's
        // `result`, and the task itself completes. (`error` is reserved for a
        // protocol-level failure during execution.)
        if let Some(t) = r
            .pointer("/result/result/content/0/text")
            .and_then(|v| v.as_str())
        {
            text = t.to_string();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        text.contains("MRTR helper") && text.contains("required-task"),
        "once in a required task must error, got: {text:?}"
    );

    handle.abort();
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

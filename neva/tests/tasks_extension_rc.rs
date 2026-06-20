//! Tasks-as-extension (MCP 2026-07-28 RC) end-to-end checks.
//!
//! Under the RC flag the tasks capability is advertised through the extensions
//! map (`capabilities.extensions["io.modelcontextprotocol/tasks"]`) instead of
//! the former top-level `capabilities.tasks` field, while the `tasks/*` wire
//! methods are unchanged. Exercised over the stateless POST-only path.
#![cfg(all(
    feature = "proto-2026-07-28-rc",
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
            .with_tasks(|t| t.with_all())
    });
    app.map_tool("ping", || async move { "pong".to_string() });
    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::new();
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
        "no top-level capabilities.tasks under the RC flag, got: {caps}"
    );

    // (b) the tasks/* wire methods are unchanged and still dispatch.
    let list = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tasks/list", "params": {}
    });
    let resp = client
        .post(&url)
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&list)
        .send()
        .await
        .expect("tasks/list failed");
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["result"]["tasks"].is_array(),
        "tasks/list should return a task array, got: {body}"
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
    // effects are just run inline (no MRTR `on_commit` needed — there is no
    // re-run); the counter below proves the resumed body ran to completion.
    TASK_COMMITS.store(0, Ordering::SeqCst);

    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app = App::new().with_options(|opt| {
        opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
            .with_tasks(|t| t.with_all())
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

    let client = reqwest::Client::new();
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

    // 1. Task-augmented call → CreateTaskResult carrying a task id.
    let r1 = post(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "name": "greet_task", "arguments": {},
            "task": { "ttl": 60000 },
            "_meta": { "clientCapabilities": { "elicitation": true } }
        }
    }))
    .await;
    let task_id = r1["result"]["task"]["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("task id present, got: {r1}"))
        .to_string();

    // 2. The tool elicits → the task suspends into input_required.
    assert!(
        wait_status("input_required", task_id.clone()).await,
        "task must enter input_required when the tool elicits"
    );

    // 3. Deliver the answer as a Response keyed by the task id (no session).
    post(serde_json::json!({
        "jsonrpc": "2.0", "id": task_id,
        "result": { "action": "accept", "content": { "name": "octocat" } }
    }))
    .await;

    // 4. The future resumes and runs to completion.
    assert!(
        wait_status("completed", task_id.clone()).await,
        "task must complete after the answer is delivered"
    );

    // 5. The final result carries the elicited value, and the resumed body ran.
    let r = post(serde_json::json!({
        "jsonrpc": "2.0", "id": 99, "method": "tasks/result",
        "params": { "taskId": task_id }
    }))
    .await;
    assert_eq!(
        r.pointer("/result/content/0/text").and_then(|v| v.as_str()),
        Some("hello octocat"),
        "final task result must carry the elicited value, got: {r}"
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
    // The MRTR `ctx.elicit` is not valid on the task substrate — it must guide
    // the author to `ctx.task().elicit(...)` rather than silently misbehave.
    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");
    let mut app = App::new().with_options(|opt| {
        opt.with_http(|http| http.bind(&addr).with_endpoint("/mcp"))
            .with_tasks(|t| t.with_all())
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

    let client = reqwest::Client::new();
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
            "_meta": { "clientCapabilities": { "elicitation": true } }
        }
    }))
    .await;
    let task_id = r1["result"]["task"]["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("task id present, got: {r1}"))
        .to_string();

    // The tool errors immediately; poll for a terminal state then read the error.
    let mut text = String::new();
    for _ in 0..100 {
        let r = post(serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "tasks/result",
            "params": { "taskId": task_id }
        }))
        .await;
        if let Some(t) = r.pointer("/result/content/0/text").and_then(|v| v.as_str()) {
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
            .with_tasks(|t| t.with_all())
    });
    app.map_tool("bad_once", |ctx: Context| async move {
        ctx.once("x", async { Ok(()) }).await?;
        Ok::<String, Error>("unreachable".into())
    })
    .with_task_support("required");

    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let client = reqwest::Client::new();
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
    let task_id = r1["result"]["task"]["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("task id present, got: {r1}"))
        .to_string();

    let mut text = String::new();
    for _ in 0..100 {
        let r = post(serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "tasks/result",
            "params": { "taskId": task_id }
        }))
        .await;
        if let Some(t) = r.pointer("/result/content/0/text").and_then(|v| v.as_str()) {
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

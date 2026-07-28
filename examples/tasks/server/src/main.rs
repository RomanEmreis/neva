use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;
use neva::prelude::*;

#[tool(task_support = "required")]
async fn endless_tool() {
    // Simulate an infinite task
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

// MCP 2026-07-28 has no task-augmented *sampling*: sampling lost its
// capability-driven server->client request and now lives on the MRTR substrate
// (`ctx.sample(key, params)`), which never mixes with the task substrate. See
// `examples/sampling` for that round-trip. Elicitation is the one input kind a
// task can await, via `ctx.task()`.
#[tool(task_support = "required")]
async fn tool_with_elicitation(mut ctx: Context, task: Meta<RelatedTaskMetadata>) -> String {
    let params = ElicitRequestParams::form("Are you sure to proceed?")
        .with_related_task(task);

    // A task does not re-run, it genuinely suspends -- so unlike the MRTR
    // `ctx.elicit(key, params)` this takes no replay key.
    let res = ctx.task().elicit(params.into()).await;

    format!("{:?}", res.unwrap().action)
}

fn main() {
    tracing_subscriber::registry()
        .with(EnvFilter::new("info"))
        .with(tracing_subscriber::fmt::layer())
        .init();
    
    App::new()
        .with_options(|opt| opt
            .with_name("Tasks Example Server")
            .with_default_http()
            .with_tasks())
        .run_blocking();
}
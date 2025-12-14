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

#[tool(task_support = "required")]
async fn tool_with_sampling(mut ctx: Context) -> String {
    let params = CreateMessageRequestParams::new()
        .with_message(SamplingMessage::from("Write a haiku."))
        .with_ttl(Some(5000));

    let res = ctx.sample(params).await;

    format!("{:?}", res.unwrap().content)
}

#[tool(task_support = "required")]
async fn tool_with_elicitation(mut ctx: Context, task: Meta<RelatedTaskMetadata>) -> String {
    let params = ElicitRequestParams::form("Are you sure to proceed?")
        .with_related_task(task);

    let res = ctx.elicit(params.into()).await;

    format!("{:?}", res.unwrap().action)
}

fn main() {
    tracing_subscriber::registry()
        .with(EnvFilter::new("info"))
        .with(tracing_subscriber::fmt::layer())
        .init();
    
    App::new()
        .with_options(|opt| opt
            .with_default_http()
            .with_tasks(|t| t.with_all()))
        .run_blocking();
}
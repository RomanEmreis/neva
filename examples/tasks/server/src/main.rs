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
        .with_message(SamplingMessage::from("Some message"))
        .with_sys_prompt("You are a helpful assistant.")
        .with_task(Some(5000));

    let res = ctx.sample(params).await;

    format!("{:?}", res.unwrap().content)
}

#[tool(task_support = "required")]
async fn tool_with_elicitation(mut ctx: Context, task: Meta<RelatedTaskMetadata>) -> String {
    let params = ElicitRequestParams::url(
        "https://www.example.com/auth", 
        "Some message")
        .with_related_task(task.into_inner());

    let res = ctx.elicit(params.into()).await;

    format!("{:?}", res.unwrap().content)
}

fn main() {
    App::new()
        .with_options(|opt| opt
            .with_timeout(std::time::Duration::from_secs(30))
            .with_default_http()
            .with_tasks(|t| t.with_all()))
        .run_blocking();
}
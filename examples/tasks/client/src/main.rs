use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;
use neva::prelude::*;

#[sampling]
async fn sampling_handler(params: CreateMessageRequestParams) -> CreateMessageResult {
    tracing::info!("Received sampling: {:?}", params);
    
    CreateMessageResult::assistant()
        .with_model("gpt-5")
        .with_content(
            r#"Winter night whispers,
Warm lights breathe through frosted glass—
Time pauses, snow listens."#)
        .end_turn()
}

#[elicitation]
async fn elicitation_handler(params: ElicitRequestParams) -> ElicitResult {
    tracing::info!("Received elicitation: {:?}", params);
    
    match params {
        ElicitRequestParams::Url(_url) => ElicitResult::accept(),
        ElicitRequestParams::Form(_form) => ElicitResult::decline()
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::registry()
        .with(EnvFilter::new("info"))
        .with(tracing_subscriber::fmt::layer())
        .init();
    
    let mut client = Client::new()
        .with_options(|opt| opt
            .with_timeout(std::time::Duration::from_secs(60))
            .with_tasks(|t| t.with_all())
            .with_default_http());
    
    client.connect().await?;

    tracing::info!("Calling tool with sampling as task...");
    let result = client.call_tool_as_task("tool_with_sampling", (), None).await;
    tracing::info!("Received result: {:?}", result);

    tracing::info!("Calling tool with elicitation as task...");
    let result = client.call_tool_as_task("tool_with_elicitation", (), None).await;
    tracing::info!("Received result: {:?}", result);

    tracing::info!("Calling an infinite tool as task...");
    let ttl = 10000; // 10 seconds
    let result = client.call_tool_as_task("endless_tool", (), Some(ttl)).await;
    tracing::info!("Received result: {:?}", result);
    
    let result = client.list_tasks(None).await?;
    tracing::info!("List of tasks: {:?}", result);
    
    client.disconnect().await
}

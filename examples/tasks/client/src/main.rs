use tracing_subscriber::prelude::*;
use neva::prelude::*;

#[sampling]
async fn sampling_handler(params: CreateMessageRequestParams) -> CreateMessageResult {
    tracing::info!("Received sampling: {:?}", params);
    
    CreateMessageResult::assistant()
        .with_model("o3-mini")
        .with_content("Some result")
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
        .with(tracing_subscriber::fmt::layer())
        .init();
    
    let mut client = Client::new()
        .with_options(|opt| opt
            .with_timeout(std::time::Duration::from_secs(60))
            .with_tasks(|t| t.with_all())
            .with_default_http());
    
    client.connect().await?;

    let result = client.call_tool_as_task("tool_with_sampling", (), None).await;
    tracing::info!("Received result: {:?}", result);

    let result = client.call_tool_as_task("tool_with_elicitation", (), None).await;
    tracing::info!("Received result: {:?}", result);

    let ttl = 10000; // 10 seconds
    let result = client.call_tool_as_task("endless_tool", (), Some(ttl)).await;
    tracing::info!("Received result: {:?}", result);
    
    let result = client.list_tasks(None).await?;
    tracing::info!("List of tasks: {:?}", result);
    
    client.disconnect().await
}

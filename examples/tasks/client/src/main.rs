use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;
use neva::prelude::*;

// No sampling handler here: MCP 2026-07-28 removed the server-push
// `sampling/createMessage` request, so there is no task-augmented sampling to
// answer. See `examples/sampling` for the MRTR form of that round-trip.
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

    tracing::info!("Calling tool with elicitation as task...");
    
    let result = client
        .task()
        .call_tool("tool_with_elicitation", ()).await;
    
    tracing::info!("Received result: {:?}", result);

    tracing::info!("Calling an infinite tool as task...");
    
    let ttl = 10000; // 10 seconds
    
    let result = client
        .task()
        .with_ttl(ttl)
        .call_tool("endless_tool", ()).await;
    
    tracing::info!("Received result: {:?}", result);
    
    let result = client.list_tasks(None).await?;
    tracing::info!("List of tasks: {:?}", result);
    
    client.disconnect().await
}

use tracing_subscriber::prelude::*;
use neva::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();
    
    let mut client = Client::new()
        .with_options(|opt| opt
            .with_timeout(std::time::Duration::from_secs(15))
            .with_tasks(|t| t.with_all())
            .with_stdio(
                "cargo",
                ["run", "--manifest-path", "examples/tasks/server/Cargo.toml"]));
    
    client.connect().await?;

    let ttl = 10000; // 10 seconds
    let result = client.call_tool_with_task("endless_tool", (), Some(ttl)).await;
    tracing::info!("Received result: {:?}", result);
    
    let result = client.list_tasks(None).await?;
    tracing::info!("List of tasks: {:?}", result);
    
    client.disconnect().await
}

use neva::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let mut client = Client::new()
        .with_options(|opt| opt
            .with_timeout(std::time::Duration::from_secs(60))
            .with_tasks(|t| t.with_all())
            .with_stdio(
                "cargo",
                ["run", "--manifest-path", "examples/tasks/server/Cargo.toml"]));
    
    client.connect().await?;
    
    let result = client.call_tool_with_task("endless_tool", (), None).await?;
    println!("Received result: {:?}", result);
    
    
    
    client.disconnect().await
}

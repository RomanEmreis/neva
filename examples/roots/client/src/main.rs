use neva::{Client, error::Error};
use tracing_subscriber::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    let mut client = Client::new()
        .with_options(|opt| opt
            .with_stdio("cargo", ["run", "--manifest-path", "examples/roots/server/Cargo.toml"])
            .with_roots(|roots| roots.with_list_changed())
            .with_mcp_version("2024-11-05"));

    client.add_root("file:///home/user/projects/my_project", "My Project");

    client.connect().await?;

    let result = client.call_tool("roots_request", None::<Vec<(&'static str, String)>>).await?;
    tracing::info!("Received result: {:?}", result.content);
    
    client.add_root("file:///home/user/projects/my_another_project", "My Another Project");
    client.publish_roots_changed().await?;
    
    client.disconnect().await
}

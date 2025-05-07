//! Run with:
//!
//! ```no_rust
//! cargo run -p example-subscription
//! ```

use neva::{Client, error::Error, types::resource::SubscribeRequestParams};
use tracing_subscriber::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();
    
    let mut client = Client::new()
        .with_options(|opt| opt
            .with_stdio("cargo", ["run", "-p", "example-updates"])
            .with_mcp_version("2024-11-05"));
    
    client.connect().await?;

    client.on_resources_changed(|_| async move {
        tracing::info!("Resources has been updated");
    });
    
    client.on_resource_changed(|n| async move {
        let params = n.params::<SubscribeRequestParams>().unwrap();
        tracing::info!("Resource: {} has been updated", params.uri); 
    });
    
    let uri = "res://test_999";
    let params = [("uri", uri)];
    let _ = client.call_tool("add_resource", Some(params)).await?;
    client.subscribe_to_resource(uri).await?;

    let params = [("uri", "res://test_1")];
    let _ = client.call_tool("remove_resource", Some(params)).await?;

    let params = [("uri", uri)];
    let _ = client.call_tool("update_resource", Some(params)).await?;
    client.unsubscribe_from_resource(uri).await?;

    // Won't get updates anymore since unsubscribed
    let _ = client.call_tool("update_resource", Some(params)).await?;

    client.disconnect().await
}

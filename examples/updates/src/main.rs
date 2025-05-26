//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector cargo run -p example-updates
//! ```

use neva::{App, Context, tool, resource};
use neva::error::Error;
use neva::types::{ReadResourceResult, Resource, Uri};

#[tool]
async fn add_resource(mut ctx: Context, uri: Uri) -> Result<(), Error> {
    ctx.add_resource(Resource::from(uri)).await
}

#[tool]
async fn remove_resource(mut ctx: Context, uri: Uri) -> Result<(), Error> {
    _ = ctx.remove_resource(uri).await?;
    Ok(())
}

#[tool]
async fn update_resource(mut ctx: Context, uri: Uri) -> Result<(), Error> {
    ctx.resource_updated(uri).await
}

#[resource(uri = "res://{name}")]
async fn get_resource(uri: Uri) -> ReadResourceResult {
    ReadResourceResult::text(
        uri.clone(), 
        "text/plain", 
        format!("Test resource {}",  uri.into_inner()))
}

#[tokio::main]
async fn main() {
    let mut app = App::new()
        .with_options(|opt| opt
            .with_stdio()
            .with_resources(|res| res
                .with_subscribe()
                .with_list_changed())
            .with_mcp_version("2024-11-05"));

    for i in 0..10 {
        app.add_resource(format!("res://test_{i}"), format!("test_{i}"));
    }

    app.run().await;
}

//! Run with:
//! 
//! ```no_rust
//! npx @modelcontextprotocol/inspector
//! 
//! cargo run -p example-http
//! ```
use neva::{App, Context, tool};
use tracing_subscriber::{filter, reload, prelude::*};
use neva::types::notification;

#[tool]
async fn remote_tool(name: String, mut ctx: Context) {
    tracing::debug!("running remote tool: {}", name);
    let roots = ctx.list_roots().await.unwrap();
    tracing::debug!("roots: {:?}", roots.roots);
}

#[tokio::main]
async fn main() {
    let (filter, handle) = reload::Layer::new(filter::LevelFilter::DEBUG);
    tracing_subscriber::registry()
        .with(filter)
        .with(notification::fmt::layer())
        .init();
    
    App::new()
        .with_options(|opt| opt
            .with_http(|http| http
                .bind("127.0.0.1:3000")
                .with_endpoint("/mcp"))
            .with_logging(handle))
        .run()
        .await;
}

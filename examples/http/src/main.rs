//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector cargo run -p example-http
//! ```
use neva::{App, tool};
use tracing_subscriber::{filter, reload, prelude::*};
use neva::types::notification::NotificationFormatter;

#[tool]
async fn remote_tool(name: String) {
    tracing::debug!("running remote tool: {}", name);
}

#[tokio::main]
async fn main() {
    let (filter, handle) = reload::Layer::new(filter::LevelFilter::DEBUG);

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer()
            .event_format(NotificationFormatter))
        .init();
    
    App::new()
        .with_options(|opt| opt
            .with_http(|http| http
                .bind("127.0.0.1:3000")
                .with_endpoint("/mcp")
            )
            .with_logging(handle))
        .run()
        .await;
}

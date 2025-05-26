//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector cargo run -p example-logging
//! ```

use neva::{App, tool};
use neva::types::notification::NotificationFormatter;
use tracing_subscriber::{filter, reload, prelude::*};

#[tool]
async fn trace_tool() {
    tracing::info!(logger = "tool", "some info message");
    tracing::warn!(logger = "tool", "some warning message");
    tracing::debug!(logger = "tool", "some debug message");
}

#[tokio::main]
async fn main() {
    // Configure logging filter
    let (filter, handle) = reload::Layer::new(filter::LevelFilter::DEBUG);
    
    // Configure logging
    tracing_subscriber::registry()
        .with(filter)                             // Specify the default logging level
        .with(tracing_subscriber::fmt::layer()
            .event_format(NotificationFormatter)) // Specify the MCP notification formatter
        .init();
    
    App::new()
        .with_options(|opt| opt
            .with_stdio()
            .with_mcp_version("2024-11-05")
            .with_logging(handle))
        .run()
        .await;
}

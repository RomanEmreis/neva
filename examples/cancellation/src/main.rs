//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector cargo run -p example-cancellation
//! ```

use neva::{App, types::{Meta, ProgressToken}, tool};
use neva::types::notification::NotificationFormatter;
use tracing_subscriber::prelude::*;

#[tool(no_schema)]
async fn long_running_task(token: Meta<ProgressToken>) {
    let mut progress = 0;
    // Simulating long-running task
    loop {
        if progress == 100 {
            break;
        }
        
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        progress += 5;
        
        tracing::info!(
            target: "progress", 
            token = %token, 
            value = %progress, 
            total = 100
        );
    }
}

#[tokio::main]
async fn main() {
    // Configure logging
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer()
            .event_format(NotificationFormatter)) // Specify the MCP notification formatter
        .init();

    let mut app = App::new()
        .with_options(|opt| opt
            .with_stdio()
            .with_mcp_version("2024-11-05"));

    map_long_running_task(&mut app);

    app.run().await;
}

//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector cargo run -p example-progress
//! ```

use neva::{App, types::{Meta, ProgressToken}, tool};
use neva::types::notification::NotificationFormatter;
use tracing_subscriber::prelude::*;

#[tool]
async fn long_running_task(token: Meta<ProgressToken>, command: String) {
    tracing::info!("Starting {command}");
    
    let mut progress = 0;
    // Simulating a long-running task
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

    tracing::info!("{command} has been successfully completed!");
}

#[tokio::main]
async fn main() {
    // Configure logging
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer()
            .event_format(NotificationFormatter)) // Specify the MCP notification formatter
        .init();

    App::new()
        .with_options(|opt| opt
            .with_stdio()
            .with_mcp_version("2024-11-05"))
        .run()
        .await;
}

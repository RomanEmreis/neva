//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector cargo run -p example-server
//! ```

use neva::prelude::*;

mod tools;
mod resources;
mod prompts;

#[handler(command = "ping")]
async fn ping_handler() {
    eprintln!("pong");
}

#[tokio::main]
async fn main() {
    App::new()
        .with_options(|opt| opt
            .with_stdio()
            .with_mcp_version("2025-06-18")
            .with_name("Sample MCP Server")
            .with_version("0.1.0.0")
            .with_tools(|tools| tools
                .with_list_changed()))
        .run()
        .await;
}

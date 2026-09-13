//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector cargo run -p example-server
//! ```

use neva::prelude::*;

mod prompts;
mod resources;
mod tools;

#[handler(command = "ping")]
async fn ping_handler() {
    eprintln!("pong");
}

// A synchronous request handler: no `async`, the response value is returned
// directly.
#[handler(command = "echo")]
fn echo_handler() -> &'static str {
    "echo"
}

#[tokio::main]
async fn main() {
    App::new()
        .with_options(|opt| {
            opt.with_stdio()
                .with_name("Sample MCP Server")
                .with_version("0.1.0.0")
                .with_tools(|tools| tools.with_list_changed())
        })
        .run()
        .await;
}

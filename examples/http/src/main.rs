//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector
//!
//! cargo run -p example-http
//! ```
use neva::prelude::*;
use tracing_subscriber::{filter, prelude::*};

#[tool]
async fn remote_tool(name: String) {
    // `logging/setLevel` is gone in MCP 2026-07-28; `notifications/message` is
    // request-scoped instead. A call that carries
    // `_meta["io.modelcontextprotocol/logLevel"]` gets these events back on its
    // own SSE reply, filtered to the level it asked for -- see
    // `notification::fmt::layer()` below. A call that asks for nothing gets none.
    tracing::debug!("running remote tool: {}", name);
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(filter::LevelFilter::DEBUG)
        .with(notification::fmt::layer())
        .init();

    App::new()
        .with_options(|opt| {
            opt.with_http(|http| http.bind("127.0.0.1:3000").with_endpoint("/mcp"))
                .with_name("Streamable HTTP Example Server")
        })
        .run()
        .await;
}

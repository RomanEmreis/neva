//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector cargo run -p example-logging
//! ```
//!
//! MCP 2026-07-28 removed the `logging/setLevel` handshake: there is no global,
//! session-wide log level to reload any more. `notifications/message` stayed --
//! as a **request-scoped** notification. The client asks per call via
//! `_meta["io.modelcontextprotocol/logLevel"]`, and while the server handles
//! that request it emits events at or above that severity and suppresses the
//! rest. With no requested level it emits none.
//!
//! Nothing about that needs wiring here: [`notification::NotificationFormatter`]
//! resolves the request-scoped level on its own. The `tracing` filter below is
//! just the usual local verbosity knob.

use neva::prelude::*;
use tracing_subscriber::{filter, prelude::*};

#[tool]
async fn trace_tool() {
    tracing::info!(logger = "tool", "some info message");
    tracing::warn!(logger = "tool", "some warning message");
    tracing::debug!(logger = "tool", "some debug message");
}

#[tokio::main]
async fn main() {
    // Configure logging
    tracing_subscriber::registry()
        .with(filter::LevelFilter::DEBUG) // Specify the default logging level
        .with(tracing_subscriber::fmt::layer().event_format(notification::NotificationFormatter)) // Specify the MCP notification formatter
        .init();

    App::new()
        .with_options(|opt| opt.with_stdio().with_name("Logging Example Server"))
        .run()
        .await;
}

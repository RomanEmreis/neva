//! An MCP Apps server: a tool with a face.
//!
//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector cargo run -p example-apps
//! ```
//!
//! Two halves, always two halves:
//!
//! 1. a **tool** that does the work and returns data, like any other tool;
//! 2. a **`ui://` resource** holding the HTML the host renders for it.
//!
//! The tool carries `_meta.ui.resourceUri`; the host fetches that resource with
//! `resources/read`, renders it in a sandboxed iframe, and pushes the tool's
//! result in over `postMessage`. This server never sends or receives a single
//! `ui/*` message -- that traffic is between the host and the iframe.

use neva::prelude::*;

mod clock;
mod report;

/// The current time, and the tool the clock app renders.
///
/// Note what it returns: a sentence, not a bare timestamp. The specification is
/// blunt about this -- a UI-bound tool **MUST** still return a meaningful
/// `content` array, because the model reads `content` and not every client has
/// an iframe. The app shows the same text; the model gets a usable answer either
/// way.
#[tool(descr = "The current time.", ui = "ui://clock/app.html")]
async fn get_time() -> String {
    format!("The time is {}.", now())
}

/// A tool the app calls and the model never sees.
///
/// `visibility = ["app"]` is what hides it. Enforcement is the host's job: the
/// server lists it in `tools/list` like any other tool, and the host keeps it
/// out of the agent's tool list.
#[tool(
    descr = "Re-read the clock.",
    ui = "ui://clock/app.html",
    visibility = ["app"]
)]
async fn refresh_clock() -> String {
    format!("The time is {}.", now())
}

fn now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();

    format!(
        "{:02}:{:02}:{:02} UTC",
        secs / 3600 % 24,
        secs / 60 % 60,
        secs % 60
    )
}

#[tokio::main]
async fn main() {
    let mut app = App::new().with_options(|opt| {
        opt.with_stdio()
            .with_name("Clock")
            .with_version("0.1.0")
            // Advertises `io.modelcontextprotocol/ui`. Without it a host has no
            // reason to look at the `_meta.ui` blocks below.
            .with_apps()
    });

    // The static-HTML path: one call registers the `ui://` read handler and
    // fills in the MIME type. The returned `&mut` stays live for the whole
    // chain -- the resource is materialized when the server starts.
    app.add_ui_resource("ui://clock/app.html", "clock", clock::CLOCK_HTML)
        .with_title("Clock")
        .with_descr("A ticking clock")
        // The app loads nothing from anywhere, so it declares nothing: an empty
        // policy is the secure default, and every origin an app touches -- its
        // own bundled scripts included -- would have to be named here.
        .with_prefers_border(true);

    app.run().await;
}

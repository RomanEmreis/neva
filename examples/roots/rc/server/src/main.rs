//! New-spec (MCP 2026-07-28 RC) roots example server.
//!
//! Under the RC there is no `roots/list` server→client *request*: the
//! capability-driven push channel is gone. The ability was re-homed onto MRTR
//! as an input-request kind, so the server asks for roots the same way it asks
//! for elicitation input — `ctx.list_roots(key)` — and the answer replays from
//! the encrypted `requestState` on the next round.
//!
//! Roots are **deprecated on arrival**: they exist for migration, hence the
//! explicit `#[allow(deprecated)]` at the call site. New tools should take the
//! paths they need as arguments instead.

use neva::prelude::*;
use tracing_subscriber::prelude::*;

#[tool]
async fn scan_workspace(mut ctx: Context) -> Result<String, Error> {
    // Everything above an input point re-runs on every round-trip, so guard
    // side effects with `once` / `memo` — here the log line proves the handler
    // really does execute twice.
    tracing::info!("🔎 scan_workspace round starting…");

    // Round 1: no answer for "dirs" yet, so this unwinds the handler and the
    // server replies `input_required` with a `roots/list` envelope.
    // Round 2: the client's `ListRootsResult` is replayed from `requestState`.
    #[allow(deprecated)]
    let roots = ctx.list_roots("dirs").await?;

    if roots.roots.is_empty() {
        return Ok("The client exposes no roots.".into());
    }

    let listed = roots
        .roots
        .iter()
        .map(|root| format!("{} ({})", root.name, root.uri))
        .collect::<Vec<_>>()
        .join(", ");

    Ok(format!("{} root(s): {listed}", roots.roots.len()))
}

#[tokio::main]
async fn main() {
    // Under RC neva does not install a global subscriber; do it here so the
    // per-round log above is visible.
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    App::new()
        // In production load a stable shared secret (env/secret store); the
        // default is an ephemeral per-process key, fine for a single instance.
        .with_request_state_secret(b"example-shared-secret")
        .with_options(|opt| {
            opt.with_name("Roots Example Server (RC)")
                .with_http(|http| http.bind("127.0.0.1:3001").with_endpoint("/mcp"))
        })
        .run()
        .await;
}

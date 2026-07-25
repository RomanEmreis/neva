//! New-spec (MCP 2026-07-28 RC) sampling example server.
//!
//! Under the RC there is no `sampling/createMessage` server->client *request*:
//! the capability-driven push channel is gone. The ability was re-homed onto
//! MRTR as an input-request kind, so the server borrows the client's model the
//! same way it asks for elicitation input -- `ctx.sample(key, params)` -- and the
//! completion replays from the encrypted `requestState` on the next round.
//!
//! Sampling is **deprecated on arrival**: it exists for migration, hence the
//! explicit `#[allow(deprecated)]` at the call site.
//!
//! Because the handler re-runs from the top on every round-trip, the expensive
//! work around the sampling point is guarded with the MRTR effect primitives --
//! exactly as it would be around an elicitation point.

use neva::prelude::*;
use neva::types::sampling::{CreateMessageRequestParams, SamplingMessage};
use tracing_subscriber::prelude::*;

#[tool]
async fn summarize_report(mut ctx: Context, topic: String) -> Result<String, Error> {
    tracing::info!("📝 summarize_report round starting...");

    // memo: the "fetch" happens once and is replayed on the second round,
    // even though everything above the sampling point re-executes.
    let report: String = ctx
        .memo("report", async {
            tracing::info!("📚 fetching source report...");
            Ok(format!(
                "Q3 numbers for {topic}: revenue up 12%, churn flat, two outages."
            ))
        })
        .await?;

    let params = CreateMessageRequestParams::new()
        .with_sys_prompt("You are a concise analyst. Answer in one sentence.")
        .with_message(SamplingMessage::user().with(format!("Summarize: {report}")))
        .with_max_tokens(200);

    // Round 1: no answer for "summary" yet, so this unwinds the handler and
    // the server replies `input_required` with a `sampling/createMessage`
    // envelope. Round 2: the client's `CreateMessageResult` is replayed.
    #[allow(deprecated)]
    let completion = ctx.sample("summary", params).await?;

    let summary = completion
        .content
        .first()
        .and_then(|content| content.as_text())
        .map(|text| text.text.clone())
        .unwrap_or_else(|| "(the client returned no text)".into());

    // on_commit: runs exactly once, on the final round.
    ctx.on_commit(async {
        tracing::info!("🗄️  summary archived");
        Ok(())
    });

    Ok(format!("[{}] {summary}", completion.model))
}

#[tokio::main]
async fn main() {
    // Under RC neva does not install a global subscriber; do it here so the
    // per-round logs above are visible.
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    App::new()
        // In production load a stable shared secret (env/secret store); the
        // default is an ephemeral per-process key, fine for a single instance.
        .with_request_state_secret(b"example-shared-secret")
        .with_options(|opt| {
            opt.with_name("Sampling Example Server (RC)")
                .with_http(|http| http.bind("127.0.0.1:3002").with_endpoint("/mcp"))
        })
        .run()
        .await;
}

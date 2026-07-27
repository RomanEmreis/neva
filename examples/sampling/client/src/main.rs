//! MCP 2026-07-28 sampling example client.
//!
//! The sampling handler is still a handler -- but it is no longer a *push*
//! endpoint. Under MCP 2026-07-28 it fulfils MRTR `sampling/createMessage` input
//! requests inside the client's round-trip loop, so the caller of `call_tool`
//! sees a single call. Registering one makes the client declare
//! `clientCapabilities.sampling`; a server may only ask for a kind the client
//! declared.
//!
//! Note the handler is wired with an explicit `map_sampling` rather than the
//! `#[sampling]` attribute: that macro belongs to the legacy push model and is
//! not available in the default (MCP 2026-07-28) build. An explicit, `#[allow(deprecated)]`
//! call is also honest about the fact that this kind is deprecated on arrival.

use neva::prelude::*;
use neva::types::sampling::{CreateMessageRequestParams, CreateMessageResult};
use tracing_subscriber::prelude::*;

/// Stands in for a real model call.
async fn complete(params: CreateMessageRequestParams) -> CreateMessageResult {
    let prompt = params
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|content| content.as_text())
        .map(|text| text.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    tracing::info!("🤖 model asked to complete: {prompt}");

    CreateMessageResult::assistant()
        .with_model("o3-mini")
        .with_content("Revenue grew 12% with steady churn, though two outages need follow-up.")
        .end_turn()
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    let mut client = Client::new().with_options(|opt| {
        opt.with_http(|http| http.bind("127.0.0.1:3002").with_endpoint("/mcp"))
    });

    // Deprecated on arrival, like the whole sampling kind -- the API stays for
    // migration.
    #[allow(deprecated)]
    client.map_sampling(complete);

    // `connect()` runs `server/discover` -- no `initialize` handshake under MCP 2026-07-28.
    client.connect().await?;

    // One call from here; the MRTR round-trips happen inside.
    let result = client
        .call_tool("summarize_report", [("topic", "EMEA")])
        .await?;
    tracing::info!("Result: {:?}", result.content);

    client.disconnect().await
}

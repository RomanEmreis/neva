//! New-spec (MCP 2026-07-28 RC) example server.
//!
//! A single `place_order` tool shows the MRTR effect primitives across an
//! elicitation round-trip: the handler re-runs top-to-bottom each round, yet
//! `memo`/`once`/`on_commit` make sure the quote is fetched once, the card is
//! charged once, and the receipt is sent once.

use neva::prelude::*;
use tracing_subscriber::prelude::*;

/// Shipping details gathered via elicitation. `#[json_schema(de)]` derives both
/// `Deserialize` and the JSON Schema used to render the elicitation form.
#[json_schema(de)]
struct Shipping {
    full_name: String,
    address: String,
}

#[tool]
async fn place_order(mut ctx: Context) -> Result<String, Error> {
    // memo: fetched once; on every replay round this returns the cached value
    // instead of running the future again.
    let quote_cents: u32 = ctx
        .memo("quote", async {
            tracing::info!("📦 fetching shipping quote…");
            Ok(1299)
        })
        .await?;

    // elicit: code above re-runs on each round-trip; the answer is replayed
    // from `requestState`, so this returns the cached answer on later rounds.
    let form = ElicitRequestParams::form(format!(
        "Shipping is ${:.2}. Please provide your shipping details:",
        quote_cents as f64 / 100.0
    ))
    .with_schema::<Shipping>();

    let ship: Shipping = ctx
        .elicit("shipping", form.into())
        .await?
        .content()
        .ok_or_else(|| Error::new(ErrorCode::InvalidParams, "shipping was declined"))?;

    // once: the charge runs at most once across all rounds.
    ctx.once("charge", async {
        tracing::info!("💳 charging card…");
        Ok(())
    })
    .await?;

    // on_commit: runs exactly once, when the handler reaches its final result.
    let who = ship.full_name.clone();
    ctx.on_commit(async move {
        tracing::info!("✉️  receipt sent to {who}");
        Ok(())
    });

    Ok(format!(
        "Order confirmed for {} shipping to {} — total ${:.2}",
        ship.full_name,
        ship.address,
        quote_cents as f64 / 100.0
    ))
}

#[tokio::main]
async fn main() {
    // Under RC neva does not install a global subscriber; do it here so the
    // effect logs above are visible.
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    App::new()
        // In production load a stable shared secret (env/secret store); the
        // default is an ephemeral per-process key, fine for a single instance.
        .with_request_state_secret(b"example-shared-secret")
        .with_options(|opt| {
            opt.with_name("MRTR Example Server")
                .with_http(|http| http.bind("127.0.0.1:3000").with_endpoint("/mcp"))
        })
        .run()
        .await;
}

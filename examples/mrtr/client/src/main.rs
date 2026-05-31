//! New-spec (MCP 2026-07-28 RC) example client.
//!
//! Connects over stateless HTTP, runs `server/discover` on `connect()`, and
//! calls `place_order`. The `#[elicitation]` handler answers the server's form
//! request transparently — the MRTR round-trips are driven inside the client.

use neva::prelude::*;
use tracing_subscriber::prelude::*;

/// Mirrors the server's `Shipping`. `#[json_schema(ser)]` derives `Serialize`
/// plus the schema the form `Validator` checks the answer against.
#[json_schema(ser)]
struct Shipping {
    full_name: String,
    address: String,
}

#[elicitation]
async fn shipping_handler(params: ElicitRequestParams) -> ElicitResult {
    match params {
        ElicitRequestParams::Form(form) => elicitation::Validator::new(form)
            .validate(Shipping {
                full_name: "Ada Lovelace".into(),
                address: "1 Analytical Way".into(),
            })
            .into(),
        // URL elicitation is part of the legacy push model, not stateless RC.
        _ => ElicitResult::decline(),
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    let mut client = Client::new().with_options(|opt| {
        opt.with_elicitation(|e| e.with_form())
            .with_http(|http| http.bind("127.0.0.1:3000").with_endpoint("/mcp"))
    });

    // `connect()` runs `server/discover`; `register_methods()` auto-wires the
    // `#[elicitation]` handler via `map_elicitation`.
    client.connect().await?;

    let result = client.call_tool("place_order", ()).await?;
    tracing::info!("Result: {:?}", result.content);

    client.disconnect().await
}

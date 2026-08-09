//! Client fixture for the official MCP conformance suite.
//!
//! The suite starts its own server, then spawns this binary with the server URL
//! as the final argument and the scenario name in `MCP_CONFORMANCE_SCENARIO`.
//! Each scenario expects a specific exchange; anything the scenario does not
//! name is left alone, so the default flow (connect, list, call) covers most of
//! them.
//!
//! ```no_rust
//! npx @modelcontextprotocol/conformance client \
//!     --command "$(pwd)/target/debug/conformance-client" --suite core
//! ```

use neva::prelude::*;
use std::time::Duration;

/// Splits the URL the suite hands us into the `host:port` and path halves the
/// client builder takes separately.
fn split_url(url: &str) -> Result<(String, String), Error> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidParams,
                format!("server URL must be http(s): {url}"),
            )
        })?;
    Ok(match rest.split_once('/') {
        Some((addr, path)) => (addr.to_owned(), format!("/{path}")),
        None => (rest.to_owned(), "/mcp".to_owned()),
    })
}

/// Scenario-specific data the suite passes in `MCP_CONFORMANCE_CONTEXT`.
fn context() -> serde_json::Value {
    std::env::var("MCP_CONFORMANCE_CONTEXT")
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or(serde_json::Value::Null)
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let url = std::env::args()
        .next_back()
        .ok_or_else(|| Error::new(ErrorCode::InvalidParams, "server URL argument is required"))?;
    let scenario =
        std::env::var("MCP_CONFORMANCE_SCENARIO").unwrap_or_else(|_| "tools_call".into());

    tracing::info!(%scenario, %url, "conformance client starting");
    tracing::debug!(context = %context(), "scenario context");

    let (addr, endpoint) = split_url(&url)?;
    let mut client = Client::new().with_options(|opt| {
        opt.with_name("neva-conformance-client")
            .with_version(env!("CARGO_PKG_VERSION"))
            .with_timeout(Duration::from_secs(20))
            .with_http(|http| http.bind(&addr).with_endpoint(&endpoint))
    });

    client.connect().await?;
    let result = run(&mut client, &scenario).await;
    client.disconnect().await?;
    result
}

/// Drives the exchange the named scenario asserts on.
async fn run(client: &mut Client, scenario: &str) -> Result<(), Error> {
    match scenario {
        // The suite's own fixture server exposes `add_numbers`; list first so
        // the recorded traffic carries both verbs, then call it.
        "tools_call"
        | "request-metadata"
        | "http-standard-headers"
        | "json-schema-2020-12-preservation"
        | "json-schema-ref-no-deref" => {
            let tools = client.list_tools(None).await?;
            tracing::info!(count = tools.tools.len(), "tools listed");
            let result = client
                .call_tool(
                    "add_numbers",
                    Some([("a", serde_json::json!(2)), ("b", serde_json::json!(3))]),
                )
                .await?;
            tracing::info!(?result, "add_numbers returned");
        }
        // Everything else: exercise the read-only surface so the scenario has
        // traffic to inspect without guessing at fixture names.
        _ => {
            let tools = client.list_tools(None).await?;
            tracing::info!(count = tools.tools.len(), "tools listed");
        }
    }
    Ok(())
}

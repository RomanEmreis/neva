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

    // Registering the handler is also what declares the capability, and MRTR
    // scenarios only get an input request if the client declared it can answer
    // one. Accepting everything is the right answer for a fixture: the suite
    // asserts on how the retry is formed, not on what was typed into it.
    client.map_elicitation(|_params: ElicitRequestParams| async {
        ElicitResult::accept().with_content(serde_json::json!({ "name": "conformance" }))
    });

    client.connect().await?;
    let result = run(&mut client, &scenario).await;
    client.disconnect().await?;
    result
}

/// Drives the exchange the named scenario asserts on.
async fn run(client: &mut Client, scenario: &str) -> Result<(), Error> {
    match scenario {
        // The harness cannot see what a client kept of a schema, so it asks for
        // it back: list the tools, then hand the observed `inputSchema` to the
        // echo tool verbatim. What arrives is what survived the round trip.
        "json-schema-2020-12-preservation" => {
            let tools = client.list_tools(None).await?;
            let observed = tools
                .tools
                .iter()
                .find(|t| t.name == "json_schema_2020_12_tool")
                .map(|t| serde_json::to_value(&t.input_schema))
                .transpose()
                .map_err(Error::from)?
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::InvalidParams,
                        "the mock server did not advertise json_schema_2020_12_tool",
                    )
                })?;

            let result = client
                .call_tool("json_schema_echo", Some([("schema", observed)]))
                .await?;
            tracing::info!(?result, "echoed the observed schema back");
        }
        // The suite's own fixture server exposes `add_numbers`; list first so
        // the recorded traffic carries both verbs, then call it.
        "tools_call"
        | "request-metadata"
        | "http-standard-headers"
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
        // The MRTR scenario's mock server checks how a retry is formed: that
        // `requestState` comes back byte-exact, that it is absent when the
        // server sent none, that the retry carries a fresh JSON-RPC id, and
        // that an unrelated call in between carries neither field. Each tool
        // drives one of those, so all four are called -- and the errors are
        // logged rather than propagated, because a tool that answers with a
        // JSON-RPC error has still produced the traffic being judged.
        "sep-2322-client-request-state" => {
            for tool in [
                "test_mrtr_echo_state",
                "test_mrtr_no_state",
                "test_mrtr_unrelated",
                "test_mrtr_no_result_type",
            ] {
                match client.call_tool(tool, None::<[(&str, &str); 0]>).await {
                    Ok(result) => tracing::info!(%tool, ?result, "tool returned"),
                    Err(err) => tracing::warn!(%tool, %err, "tool failed"),
                }
            }
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

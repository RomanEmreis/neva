//! The client half of the Keycloak walkthrough -- see README.md in this
//! directory for the full setup.
//!
//! Uses the client pre-registered by the realm import
//! (`neva-mcp-client`, redirect `http://127.0.0.1:8919/callback`), so
//! the loopback listener is pinned to that port. On the first request
//! the browser opens Keycloak's login page -- sign in as `demo` / `demo`.
//!
//! ```no_rust
//! cargo run -p example-oauth-with-keycloak --bin keycloak-client
//! ```
use neva::auth::oauth::LoopbackHandler;
use neva::prelude::*;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt().init();

    let mut client = Client::new().with_options(|opt| {
        opt.with_http(|http| {
            http.bind("127.0.0.1:3000").with_oauth(|oauth| {
                oauth
                    .with_client_id("neva-mcp-client")
                    // local Keycloak runs over plain http
                    .require_https(false)
                    // the realm registers exactly this redirect URI
                    .with_handler(LoopbackHandler::new().with_port(8919))
            })
        })
        // the first request waits for the browser login
        .with_timeout(Duration::from_secs(300))
    });

    client.connect().await?;

    tracing::info!("--- CALL whoami ---");
    let result = client.call_tool("whoami", ()).await?;
    tracing::info!("{:?}", result.content);

    // `demo` carries the `admin` realm role, so the gated tool works too
    tracing::info!("--- CALL admin_report ---");
    let result = client.call_tool("admin_report", ("name", "q3")).await?;
    tracing::info!("{:?}", result.content);

    client.disconnect().await
}

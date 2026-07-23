//! An MCP client with automatic OAuth 2.1 authorization: the first `401`
//! drives discovery, dynamic client registration and the
//! authorization-code + PKCE flow through the system browser; the token
//! is attached (and refreshed) transparently afterwards.
//!
//! Start a protected server first (see `examples/oauth-server` or
//! `examples/oauth-with-keycloak`), then:
//!
//! ```no_rust
//! cargo run -p example-oauth-client
//! ```
use neva::prelude::*;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt().init();

    let mut client = Client::new().with_options(|opt| {
        opt.with_http(|http| {
            http.bind("127.0.0.1:3000")
                // Everything is optional: without a client id the client
                // registers dynamically (RFC 7591); without scopes the
                // server's advertised `scopes_supported` are requested;
                // the default handler opens the system browser and
                // catches the redirect on an ephemeral loopback port.
                //
                // Against an issuer that requires a pre-registered
                // client and a fixed redirect port (e.g. the Keycloak
                // example):
                //
                //   .with_oauth(|oauth| oauth
                //       .with_client_id("neva-mcp-client")
                //       .require_https(false)
                //       .with_handler(LoopbackHandler::new().with_port(8919)))
                .with_oauth(|oauth| oauth)
        })
        // The first request may wait for the user to finish in the
        // browser -- give it more than the default 10 seconds.
        .with_timeout(Duration::from_secs(300))
    });

    client.connect().await?;

    tracing::info!("--- LIST TOOLS ---");
    let tools = client.list_tools(None).await?;
    for tool in tools.tools.iter() {
        tracing::info!("- {}", tool.name);
    }

    tracing::info!("--- CALL TOOL ---");
    let result = client.call_tool("whoami", ()).await?;
    tracing::info!("{:?}", result.content);

    client.disconnect().await
}

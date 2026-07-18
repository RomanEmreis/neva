//! The resource-server half of the Keycloak walkthrough — see README.md
//! in this directory for the full setup.
//!
//! Minimal configuration on purpose: pointing bearer auth at the issuer
//! is enough. Tokens are validated against Keycloak's JWKS, the token's
//! `aud` is bound to `http://127.0.0.1:3000/mcp` (granted by the realm's
//! audience mapper), and the RFC 9728 metadata document is derived from
//! the issuer and served on the well-known path automatically.
//!
//! ```no_rust
//! cargo run -p example-oauth-with-keycloak --bin keycloak-server
//! ```
use neva::prelude::*;

const ISSUER: &str = "http://localhost:8080/realms/neva";

/// A tool available to any authenticated caller
#[tool]
async fn whoami() -> &'static str {
    "an authenticated caller"
}

/// A tool available only to callers with the `admin` realm role
/// (delivered in the `roles` claim by the realm's role mapper)
#[tool(roles = ["admin"])]
async fn admin_report(name: String) -> String {
    format!("confidential report: {name}")
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().init();

    App::new()
        .with_options(|opt| {
            opt.with_name("Keycloak-protected MCP Server")
                .with_http(|http| {
                    http.bind("127.0.0.1:3000").with_auth(|auth| {
                        auth.with_oauth(|oauth| {
                            oauth
                                .with_issuer(ISSUER)
                                // local Keycloak runs over plain http
                                .with_config(|cfg| {
                                    cfg.with_client_config(|c| c.require_https(false))
                                })
                        })
                    })
                })
        })
        .run()
        .await;
}

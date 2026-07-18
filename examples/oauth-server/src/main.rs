//! An MCP server protected as an OAuth 2.1 resource server: bearer
//! tokens are validated against the issuer's JWKS, the RFC 9728
//! Protected Resource Metadata document is served on the well-known
//! path, and 401 challenges point clients at it.
//!
//! Run with:
//!
//! ```no_rust
//! # Any OAuth 2.1 / OIDC issuer:
//! OAUTH_ISSUER=https://auth.example.com cargo run -p example-oauth-server
//!
//! # A local issuer over plain http (e.g. Keycloak from
//! # examples/oauth-with-keycloak):
//! OAUTH_ISSUER=http://localhost:8080/realms/neva OAUTH_ALLOW_HTTP=1 \
//!     cargo run -p example-oauth-server
//! ```
//!
//! Then connect with `cargo run -p example-oauth-client` (or MCP
//! Inspector) - the client discovers everything from the 401 challenge.
use neva::prelude::*;

/// A tool available to any authenticated caller
#[tool]
async fn whoami() -> &'static str {
    "an authenticated caller"
}

/// A tool available only to admins
#[tool(roles = ["admin"])]
async fn admin_report(name: String) -> String {
    format!("confidential report: {name}")
}

#[tokio::main]
async fn main() {
    let issuer = std::env::var("OAUTH_ISSUER").expect("OAUTH_ISSUER must be set");
    let allow_http = std::env::var("OAUTH_ALLOW_HTTP").is_ok();

    tracing_subscriber::fmt().init();

    App::new()
        .with_options(|opt| {
            opt.with_name("OAuth Server Example").with_http(|http| {
                http.bind("127.0.0.1:3000")
                    // Explicit RFC 9728 document. Optional: with
                    // `with_auth(...with_oauth(...))` alone the document
                    // is derived from the issuer automatically - spelled
                    // out here to show the knobs (scopes, extra fields,
                    // and `with_resource(...)` for reverse proxies).
                    .with_oauth_metadata(|oauth| {
                        oauth
                            .with_authorization_servers([issuer.as_str()])
                            .with_scopes(["mcp:tools"])
                    })
                    .with_auth(|auth| {
                        auth.with_oauth(|oauth| {
                            let oauth = oauth.with_issuer(issuer.as_str());
                            // Plain-http issuers are for local development
                            // only - discovery rejects them by default.
                            if allow_http {
                                oauth.with_config(|cfg| {
                                    cfg.with_client_config(|c| c.require_https(false))
                                })
                            } else {
                                oauth
                            }
                        })
                    })
            })
        })
        .run()
        .await;
}

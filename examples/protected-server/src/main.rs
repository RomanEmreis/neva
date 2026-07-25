//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector
//!
//! # Static decoding key (HS256):
//! JWT_SECRET=a-string-secret-at-least-256-bits-long cargo run -p example-protected-server
//!
//! # OAuth 2.1/OIDC issuer mode -- tokens are validated against the
//! # issuer's JWKS, and /.well-known/oauth-protected-resource/mcp is
//! # served automatically:
//! OAUTH_ISSUER=https://auth.example.com cargo run -p example-protected-server
//! ```
use neva::prelude::*;
use tracing_subscriber::{filter, prelude::*, reload};

/// A tool that allowed to everyone
#[tool]
async fn remote_tool(name: String) {
    tracing::debug!("running remote tool: {}", name);
}

/// A tool that allowed only to admins
#[tool(roles = ["admin"])]
async fn admin_tool(name: String) {
    tracing::debug!("running admin tool: {}", name);
}

/// A prompt that allowed only to admins with the `read` permission
#[prompt(roles = ["admin"], permissions = ["read"])]
async fn restricted_prompt(name: String) -> (&'static str, &'static str) {
    tracing::debug!("getting restricted prompt: {}", name);
    ("this is the restricted prompt", "admin")
}

/// A resource that allowed only with the `read` permission
#[resource(uri = "res://restricted/{name}", permissions = ["read"])]
async fn restricted_resource(uri: Uri, name: String) -> (String, String) {
    tracing::debug!("requested resource: {}", name);
    (uri.to_string(), name)
}

#[tokio::main]
async fn main() {
    let issuer = std::env::var("OAUTH_ISSUER").ok();
    let secret = std::env::var("JWT_SECRET").ok();

    let (filter, handle) = reload::Layer::new(filter::LevelFilter::DEBUG);
    tracing_subscriber::registry()
        .with(filter)
        .with(notification::fmt::layer())
        .init();

    #[allow(deprecated)]
    App::new()
        .with_options(|opt| {
            opt.with_name("Protected Server Example")
                .with_http(|http| {
                    http.with_auth(|auth| match (&issuer, &secret) {
                        // OAuth issuer mode: keys come from the issuer's
                        // JWKS; the token's `aud` is bound to this server's
                        // canonical URI and its `iss` to the issuer, and the
                        // RFC 9728 metadata document is derived and served
                        // automatically. For a local issuer over plain http
                        // (e.g. Keycloak on localhost), relax the discovery
                        // client with:
                        //   .with_config(|cfg| cfg
                        //       .with_client_config(|c| c.require_https(false)))
                        (Some(issuer), _) => auth.with_oauth(|oauth| oauth.with_issuer(issuer)),
                        // Static decoding key (HS256) -- the pre-OAuth setup.
                        (None, Some(secret)) => auth
                            .validate_exp(false)
                            .with_aud(["some aud"])
                            .with_iss(["some issuer"])
                            .set_decoding_key(secret.as_bytes()),
                        (None, None) => panic!("Set OAUTH_ISSUER or JWT_SECRET"),
                    })
                })
                .with_logging(handle)
        })
        .run()
        .await;
}

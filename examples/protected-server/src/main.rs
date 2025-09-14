//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector
//!
//! JWT_SECRET=a-string-secret-at-least-256-bits-long cargo run -p protected-server
//! ```
use neva::prelude::*;
use tracing_subscriber::{filter, reload, prelude::*};

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
    let secret = std::env::var("JWT_SECRET")
        .expect("JWT_SECRET must be set");

    let (filter, handle) = reload::Layer::new(filter::LevelFilter::DEBUG);
    tracing_subscriber::registry()
        .with(filter)
        .with(notification::fmt::layer())
        .init();

    App::new()
        .with_options(|opt| opt
            .with_http(|http| http
                .with_auth(|auth| auth
                    .validate_exp(false)
                    .with_aud(["some aud"])
                    .with_iss(["some issuer"])
                    .set_decoding_key(secret.as_bytes())))
            .with_logging(handle))
        .run()
        .await;
}

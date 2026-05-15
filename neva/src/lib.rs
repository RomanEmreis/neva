//! # Neva
//! Easy configurable MCP server and client SDK for Rust
//!
//! ## Dependencies
//! ```toml
//! [dependencies]
//! neva = { version = "0.2.1", features = ["full"] }
//! tokio = { version = "1", features = ["full"] }
//! ```
//!
//! ## Example Server
//! ```no_run
//! # #[cfg(feature = "server")] {
//! use neva::App;
//!
//! #[tokio::main]
//! async fn main() {
//!     let mut app = App::new()
//!         .with_options(|opt| opt
//!             .with_stdio());
//!
//!     app.map_tool("hello", |name: String| async move {
//!         format!("Hello, {name}!")
//!     });
//!
//!     app.run().await;
//! }
//! # }
//! ```
//! # Example Client
//! ```no_run
//! # #[cfg(feature = "client")] {
//! use std::time::Duration;
//! use neva::{Client, error::Error};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Error> {
//!     let mut client = Client::new()
//!         .with_options(|opt| opt
//!             .with_stdio("npx", ["-y", "@modelcontextprotocol/server-everything"]));
//!
//!     client.connect().await?;
//!
//!     // Call a tool
//!     let args = [("message", "Hello MCP!")];
//!     let result = client.call_tool("echo", Some(args)).await?;
//!     println!("{:?}", result.content);
//!
//!     client.disconnect().await
//! }
//! # }
//! ```

#[cfg(feature = "server")]
pub use app::{App, context::Context};
#[cfg(feature = "client")]
pub use client::Client;

#[cfg(feature = "server")]
pub mod app;
#[cfg(feature = "client")]
pub mod client;
pub mod commands;
#[cfg(feature = "di")]
pub mod di;
pub mod error;
#[cfg(feature = "macros")]
pub mod macros;
#[cfg(feature = "server")]
pub mod middleware;
pub mod shared;
#[cfg(any(feature = "server", feature = "client"))]
pub mod transport;
pub mod types;

#[cfg(feature = "macros")]
pub use neva_macros::json_schema;
#[cfg(feature = "server-macros")]
pub use neva_macros::{completion, handler, prompt, resource, resources, tool};
#[cfg(feature = "client-macros")]
pub use neva_macros::{elicitation, sampling};

pub(crate) const SDK_NAME: &str = "neva";
#[cfg(any(feature = "server", feature = "client"))]
pub(crate) const PROTOCOL_VERSIONS: [&str; 4] =
    ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

#[cfg(feature = "http-server")]
pub mod auth {
    //! Authentication utilities — neva's engine-neutral [`Claims`] trait
    //! + (under the Volga adapter) the bearer-auth configuration types.

    /// `Claims` is neva's engine-neutral trait for typed per-tool
    /// authorization. Implement this for your custom claims type to enable
    /// `with_roles` / `with_permissions` gating regardless of which HTTP
    /// engine delivered the request.
    ///
    /// The Volga adapter's `DefaultClaims` already implements both this
    /// trait and `volga::auth::AuthClaims`, so the same per-tool validator
    /// runs across every engine.
    pub use crate::transport::http::core::types::Claims;

    /// `DefaultClaims` is a pre-built [`Claims`] impl matching the JWT
    /// standard claim names. Engine-agnostic — under the Volga adapter
    /// it additionally implements `volga::auth::AuthClaims` so it can
    /// be fed straight into Volga's bearer-auth pipeline.
    pub use crate::transport::http::core::types::DefaultClaims;

    /// `AuthConfig` is the Volga-flavored builder used with
    /// `HttpServer::with_auth(...)`. Available only under the Volga adapter.
    #[cfg(feature = "http-server-volga")]
    pub use crate::transport::http::server::volga::auth_config::AuthConfig;

    /// Volga's claims trait, re-exported for users who need to plug a
    /// custom claims type into Volga's `Authorizer<C>`. For neva's own
    /// per-tool checks, implement [`Claims`] instead — that one is
    /// engine-neutral.
    #[cfg(feature = "http-server-volga")]
    pub use volga::auth::AuthClaims;

    // Volga's `Claims` is a derive macro in the macro namespace; re-export
    // it as `ClaimsDerive` so it doesn't collide with the `Claims` trait
    // alias above (which lives in the type namespace).
    #[cfg(feature = "http-server-volga")]
    pub use volga::auth::{Algorithm, Authorizer, Claims as ClaimsDerive};
}

pub mod json {
    //! JSON utilities

    #[doc(hidden)]
    pub use schemars;
    pub use schemars::JsonSchema;
}

pub mod prelude {
    //! Prelude with commonly used items

    pub use crate::error::*;
    pub use crate::json::*;
    pub use crate::types::*;

    #[cfg(feature = "http-server")]
    pub use crate::auth::Claims;
    #[cfg(feature = "http-server")]
    pub use crate::transport::HttpServer;
    #[cfg(all(feature = "http-server", feature = "server-tls"))]
    pub use crate::transport::http::{DevCertMode, TlsConfig};

    #[cfg(feature = "server")]
    pub use crate::app::{App, context::Context, options};
    #[cfg(feature = "server")]
    pub use crate::middleware::{MwContext, Next};

    #[cfg(feature = "client")]
    pub use crate::client::Client;

    #[cfg(feature = "macros")]
    pub use crate::json_schema;
    #[cfg(feature = "server-macros")]
    pub use crate::{completion, handler, prompt, resource, resources, tool};
    #[cfg(feature = "client-macros")]
    pub use crate::{elicitation, sampling};

    #[cfg(feature = "http-server")]
    pub use crate::auth::*;

    #[cfg(feature = "di")]
    pub use crate::di::Dc;

    #[cfg(feature = "tasks")]
    pub use crate::shared::TaskApi;
}

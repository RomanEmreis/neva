//! # Neva
//! Easy configurable MCP server and client SDK for Rust
//! 
//! ## Dependencies
//! ```toml
//! [dependencies]
//! neva = { version = "0.1.5", features = ["full"] }
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

pub mod types;
#[cfg(any(feature = "server", feature = "client"))]
pub mod transport;
pub mod error;
pub mod shared;
#[cfg(feature = "server")]
pub mod app;
#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "macros")]
pub mod macros;

#[cfg(feature = "server-macros")]
pub use neva_macros::{tool, prompt, resource, resources, handler};
#[cfg(feature = "client-macros")]
pub use neva_macros::{sampling, elicitation};
#[cfg(feature = "macros")]
pub use neva_macros::json_schema;

pub(crate) const SDK_NAME: &str = "neva";
#[cfg(any(feature = "server", feature = "client"))]
pub(crate) const PROTOCOL_VERSIONS: [&str; 3] = [
    "2024-11-05", 
    "2025-03-26",
    "2025-06-18"
];

#[cfg(feature = "http-server")]
pub mod auth {
    pub use volga::auth::{Algorithm, Authorizer, Claims};
    pub use crate::transport::http::server::{AuthConfig, DefaultClaims};
}

pub mod json {
    pub use schemars::JsonSchema;
    #[doc(hidden)]
    pub use schemars;
}

/// List of commands
pub mod commands {
    pub const INIT: &str = "initialize";
    pub const PING: &str = "ping";
}

pub mod prelude {
    pub use crate::types::*;
    pub use crate::error::*;
    pub use crate::json::*;

    #[cfg(feature = "server")]
    pub use crate::app::{App, context::Context, options};
    #[cfg(feature = "client")]
    pub use crate::client::Client;
    
    #[cfg(feature = "server-macros")]
    pub use crate::{tool, prompt, resource, resources, handler};
    #[cfg(feature = "client-macros")]
    pub use crate::{sampling, elicitation};
    #[cfg(feature = "macros")]
    pub use crate::json_schema;

    #[cfg(feature = "http-server")]
    pub use crate::auth::*;
}

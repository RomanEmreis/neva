//! # Neva
//! Easy configurable MCP server SDK for Rust
//! 
//! ## Dependencies
//! ```toml
//! [dependencies]
//! neva = { version = "0.0.4", features = ["full"] }
//! tokio = { version = "1", features = ["full"] }
//! ```
//! 
//! ## Example
//! 
//! ```no_run
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
//! ```

pub use app::{App, options};

pub mod app;
pub mod types;
pub mod transport;
pub mod error;

#[cfg(feature = "macros")]
pub use neva_macros::*;

pub(crate) const SERVER_NAME: &str = "neva";
pub(crate) const PROTOCOL_VERSIONS: [&str; 2] = [
    "2024-11-05", 
    "2025-03-26"
];

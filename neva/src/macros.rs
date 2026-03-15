//! Utilities for macros

pub use inventory;

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "server")]
pub mod server;

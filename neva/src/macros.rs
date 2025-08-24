//! Utilities for macros

pub use inventory;

#[cfg(feature = "server")]
pub mod server;
#[cfg(feature = "client")]
pub mod client;

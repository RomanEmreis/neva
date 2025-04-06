pub use app::{App, options};

pub mod app;
pub mod types;
pub mod transport;
pub mod error;

pub use neva_macros::*;

pub(crate) const SERVER_NAME: &str = "neva";
pub(crate) const PROTOCOL_VERSIONS: [&str; 2] = [
    "2024-11-05", 
    "2025-03-26"
];

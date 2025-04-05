pub use app::{App, options};

pub mod app;
pub mod types;
pub mod transport;
pub mod error;

pub use neva_macros::*;

pub(crate) const SERVER_NAME: &str = "neva";
pub(crate) const PROTOCOL_VERSION: &str = "2024-11-05";

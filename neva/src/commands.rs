//! List of built-in commands supported by the MCP protocol

/// Command name for initializing the server
pub const INIT: &str = "initialize";

/// Command name for pinging the server
pub const PING: &str = "ping";

/// Command name for stateless capability discovery (MCP 2026-07-28 RC).
#[cfg(feature = "proto-2026-07-28-rc")]
pub const DISCOVER: &str = "server/discover";

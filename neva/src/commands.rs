//! List of built-in commands supported by the MCP protocol

/// Command name for initializing the server
pub const INIT: &str = "initialize";

/// Command name for pinging the server
///
/// Removed in MCP 2026-07-28: the stateless transport has no connection to
/// keep alive, so liveness is a transport concern rather than an RPC.
#[cfg(feature = "legacy-spec")]
pub const PING: &str = "ping";

/// Command name for stateless capability discovery (MCP 2026-07-28).
#[cfg(not(feature = "legacy-spec"))]
pub const DISCOVER: &str = "server/discover";

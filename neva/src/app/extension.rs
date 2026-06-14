//! Protocol extensions (MCP 2026-07-28 RC).
//!
//! The RC reclassifies several former core features (starting with Tasks) as
//! *extensions*: each is identified by a reverse-DNS id, advertises a
//! capability value under `capabilities.extensions[id]`, and brings its own
//! request handlers. This module defines the [`Extension`] trait that wires
//! such a feature into an [`App`]; concrete extensions live in submodules
//! (e.g. the built-in [`TasksExtension`]).

use super::App;

#[cfg(feature = "tasks")]
mod tasks;

#[cfg(feature = "tasks")]
pub use tasks::TasksExtension;

/// A protocol extension (MCP 2026-07-28 RC).
///
/// An extension contributes a capability value (surfaced in `server/discover`
/// under `capabilities.extensions`) and registers its request handlers.
/// Register one with [`App::with_extension`].
///
/// # Example
///
/// ```rust,ignore
/// use neva::App;
/// use neva::app::extension::Extension;
///
/// struct Echo;
/// impl Extension for Echo {
///     fn id(&self) -> &'static str { "com.example/echo" }
///     fn capability(&self) -> serde_json::Value { serde_json::json!({}) }
///     fn register(self, app: &mut App) {
///         app.map_handler("com.example/echo", |msg: String| async move { msg });
///     }
/// }
///
/// let app = App::new().with_extension(Echo);
/// ```
pub trait Extension {
    /// Reverse-DNS identifier, e.g. `io.modelcontextprotocol/tasks`.
    fn id(&self) -> &'static str;

    /// The capability value advertised under `capabilities.extensions[id]`.
    fn capability(&self) -> serde_json::Value;

    /// Registers this extension's request handlers into `app`.
    fn register(self, app: &mut App);
}

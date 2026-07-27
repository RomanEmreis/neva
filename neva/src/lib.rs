//! # Neva
//! Easy configurable MCP server and client SDK for Rust
//!
//! ## Dependencies
//! ```toml
//! [dependencies]
//! neva = { version = "0.3.3", features = ["full"] }
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

#[cfg(all(feature = "server", not(feature = "legacy-spec")))]
pub use app::extension::Extension;
#[cfg(all(feature = "server", not(feature = "legacy-spec"), feature = "tasks"))]
pub use app::extension::TasksExtension;
#[cfg(all(feature = "server", not(feature = "legacy-spec")))]
pub use app::mrtr_store::{InMemoryStateStore, RequestStateStore};
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

#[cfg(feature = "client-macros")]
pub use neva_macros::elicitation;
#[cfg(feature = "macros")]
pub use neva_macros::json_schema;
#[cfg(all(feature = "client-macros", feature = "legacy-spec"))]
pub use neva_macros::sampling;
#[cfg(feature = "server-macros")]
pub use neva_macros::{completion, handler, prompt, resource, resources, tool};

pub(crate) const SDK_NAME: &str = "neva";
#[cfg(any(feature = "server", feature = "client"))]
pub(crate) const PROTOCOL_VERSIONS: &[&str] = &[
    "2024-11-05",
    "2025-03-26",
    "2025-06-18",
    "2025-11-25",
    #[cfg(not(feature = "legacy-spec"))]
    LATEST_PROTOCOL_VERSION,
];

/// The MCP protocol version this build targets by default (MCP 2026-07-28).
///
/// Compiled out under `legacy-spec`, where the newest supported generation is
/// `2025-11-25`.
#[cfg(not(feature = "legacy-spec"))]
pub(crate) const LATEST_PROTOCOL_VERSION: &str = "2026-07-28";

// Mutual-exclusion guard for protocol-generation flags.
//
// The default build targets MCP 2026-07-28; `legacy-spec` swaps in the
// pre-2026-07-28 generations instead. One pairwise `all(...)` lives in the
// `any(...)` body for every pair of mutually exclusive generation flags.
// Today only `legacy-spec` exists, so the body is empty and the guard is
// dormant (`cfg(any())` with no operands evaluates to `false`). When a future
// `proto-*` generation flag is introduced, append
// `all(feature = "legacy-spec", feature = "proto-A")` to the list.
#[cfg(any(
    // all(feature = "legacy-spec", feature = "proto-2027-XX-XX"),
))]
compile_error!("Only one protocol-generation feature flag may be enabled per build");

#[cfg(any(feature = "http-server", feature = "client-oauth"))]
pub mod auth {
    //! Authentication utilities: neva's engine-neutral `Claims` trait
    //! (under the HTTP server features),
    //! the bearer-auth configuration types (under the Volga adapter),
    //! and the OAuth 2.1 building blocks for both sides of the
    //! Streamable HTTP transport (under the OAuth features).

    /// `Claims` is neva's engine-neutral trait for typed per-tool
    /// authorization. Implement this for your custom claims type to enable
    /// `with_roles` / `with_permissions` gating regardless of which HTTP
    /// engine delivered the request.
    ///
    /// The Volga adapter's `DefaultClaims` already implements both this
    /// trait and `volga::auth::AuthClaims`, so the same per-tool validator
    /// runs across every engine.
    ///
    /// # Engine contract
    ///
    /// An [`HttpEngine`](crate::transport::http::core::engine::HttpEngine)
    /// adapter that wants protected tools/prompts/resources to authorize
    /// must wrap its decoded claims in `Arc<dyn Claims>` and insert it
    /// into the inbound request's extensions before calling the
    /// `dispatch_post` helper:
    ///
    /// ```rust,ignore
    /// use std::sync::Arc;
    /// use neva::auth::Claims;
    ///
    /// // in the engine's POST route, after decoding the bearer token:
    /// let claims: Arc<dyn Claims> = Arc::new(my_decoded_claims);
    /// neutral_req.extensions_mut().insert(claims);
    /// ```
    #[cfg(feature = "http-server")]
    pub use crate::transport::http::core::types::Claims;

    /// `DefaultClaims` is a pre-built [`Claims`] impl matching the JWT
    /// standard claim names. Engine-agnostic -- under the Volga adapter
    /// it additionally implements `volga::auth::AuthClaims` so it can
    /// be fed straight into Volga's bearer-auth pipeline.
    #[cfg(feature = "http-server")]
    pub use crate::transport::http::core::types::DefaultClaims;

    /// `AuthConfig` is the Volga-flavored builder used with
    /// `HttpServer::with_auth(...)`. Available only under the Volga adapter.
    #[cfg(feature = "http-server-volga")]
    pub use crate::transport::http::server::volga::auth_config::AuthConfig;

    /// `OAuthConfig` describes the OAuth 2.1/OIDC issuer whose keys
    /// validate bearer tokens, used with `AuthConfig::with_oauth(...)`.
    /// Available only under the Volga adapter with `server-oauth`.
    #[cfg(all(feature = "http-server-volga", feature = "server-oauth"))]
    pub use crate::transport::http::server::volga::auth_config::OAuthConfig;

    /// Volga's claims trait, re-exported for users who need to plug a
    /// custom claims type into Volga's `Authorizer<C>`. For neva's own
    /// per-tool checks, implement [`Claims`] instead -- that one is
    /// engine-neutral.
    #[cfg(feature = "http-server-volga")]
    pub use volga::auth::AuthClaims;

    // Volga's `Claims` is a derive macro in the macro namespace; re-export
    // it as `ClaimsDerive` so it doesn't collide with the `Claims` trait
    // alias above (which lives in the type namespace).
    #[cfg(feature = "http-server-volga")]
    pub use volga::auth::{Algorithm, Authorizer, Claims as ClaimsDerive};

    #[cfg(any(feature = "server-oauth", feature = "client-oauth"))]
    pub mod oauth {
        //! OAuth 2.1 / OIDC building blocks.
        //!
        //! Server side (`server-oauth`): `OAuthResourceOptions`
        //! configures the RFC 9728 Protected Resource Metadata document
        //! through `HttpServer::with_oauth_metadata`; the protocol-level
        //! types are re-exported from
        //! [`volga-oauth-core`](https://docs.rs/volga-oauth-core) --
        //! a crate with no HTTP I/O and no dependency on the Volga
        //! framework, so they are available to every `HttpEngine`.
        //!
        //! Client side (`client-oauth`): the pluggable
        //! `AuthorizationHandler` contract with the default
        //! `LoopbackHandler`, plus the `TokenStore` persistence
        //! abstraction -- configured through `HttpClient::with_oauth`.

        #[cfg(feature = "server-oauth")]
        pub use crate::transport::http::core::oauth::{
            BearerChallenge, OAuthError, OAuthErrorCode, OAuthResourceOptions,
            ProtectedResourceMetadata, WELL_KNOWN_PROTECTED_RESOURCE, canonicalize_resource_uri,
            protected_resource_metadata_url,
        };

        #[cfg(feature = "client-oauth")]
        pub use crate::transport::http::client::oauth::{
            AuthorizationHandler, CallbackParams, InMemoryTokenStore, LoopbackHandler,
            OAuthClientConfig, TokenSet, TokenStore,
        };
    }
}

pub mod json {
    //! JSON utilities

    #[doc(hidden)]
    pub use schemars;
    pub use schemars::JsonSchema;
}

/// Internal re-exports used by `neva_macros`-generated code. Not public API.
#[cfg(not(feature = "legacy-spec"))]
#[doc(hidden)]
pub mod __macro_support {
    pub use crate::types::schema_2020::{
        SchemaProbe, ViaFallback, ViaJsonSchema, object_schema, primitive_subschema,
    };
}

pub mod prelude {
    //! Prelude with commonly used items

    pub use crate::error::*;
    pub use crate::json::*;
    pub use crate::shared::BoxFuture;
    pub use crate::types::*;

    #[cfg(feature = "http-server-volga")]
    pub use crate::auth::AuthConfig;
    #[cfg(feature = "server-oauth")]
    pub use crate::auth::oauth::OAuthResourceOptions;
    #[cfg(feature = "http-server")]
    pub use crate::auth::{Claims, DefaultClaims};

    #[cfg(feature = "http-server")]
    #[allow(deprecated)]
    pub use crate::transport::SseResponse;
    #[cfg(all(feature = "http-server", feature = "server-tls"))]
    pub use crate::transport::http::{DevCertMode, TlsConfig};
    #[cfg(feature = "http-server")]
    pub use crate::transport::{
        HttpContext, HttpEngine, HttpRequest, HttpResponse, HttpServer, StreamResponse, handlers,
    };

    #[cfg(all(feature = "server", not(feature = "legacy-spec")))]
    pub use crate::app::extension::Extension;
    #[cfg(all(feature = "server", not(feature = "legacy-spec"), feature = "tasks"))]
    pub use crate::app::extension::TasksExtension;
    #[cfg(feature = "server")]
    pub use crate::app::{App, context::Context, options};
    #[cfg(feature = "server")]
    pub use crate::middleware::{MwContext, Next};

    #[cfg(feature = "client")]
    pub use crate::client::Client;

    #[cfg(feature = "client-macros")]
    pub use crate::elicitation;
    #[cfg(feature = "macros")]
    pub use crate::json_schema;
    #[cfg(all(feature = "client-macros", feature = "legacy-spec"))]
    pub use crate::sampling;
    #[cfg(feature = "server-macros")]
    pub use crate::{completion, handler, prompt, resource, resources, tool};

    #[cfg(feature = "di")]
    pub use crate::di::Dc;

    #[cfg(feature = "tasks")]
    pub use crate::shared::TaskApi;
}

#[cfg(test)]
#[cfg(any(feature = "server", feature = "client"))]
mod proto_versions_tests {
    use super::PROTOCOL_VERSIONS;

    #[test]
    fn latest_version_listed_unless_legacy() {
        let has_latest = PROTOCOL_VERSIONS.contains(&"2026-07-28");
        let legacy = cfg!(feature = "legacy-spec");
        assert_eq!(
            has_latest, !legacy,
            "2026-07-28 must be advertised unless `legacy-spec` is on"
        );
    }

    #[test]
    fn stable_versions_always_listed() {
        // Older versions are PROTOCOL_VERSIONS minus the 2026-07-28 entry (when enabled).
        // Future stable additions land in PROTOCOL_VERSIONS and are automatically
        // covered by this test -- no need to update the test when new versions
        // are advertised.
        let stable: Vec<_> = PROTOCOL_VERSIONS
            .iter()
            .filter(|v| **v != "2026-07-28")
            .copied()
            .collect();
        assert!(
            !stable.is_empty(),
            "PROTOCOL_VERSIONS must always advertise at least one stable version"
        );
        // The set must include 2024-11-05 (the inaugural MCP version) -- this is
        // a stronger invariant: even if we ever retire intermediate versions,
        // the original SHOULD remain for backwards compatibility.
        assert!(
            stable.contains(&"2024-11-05"),
            "PROTOCOL_VERSIONS must always advertise the inaugural MCP version 2024-11-05"
        );
    }
}

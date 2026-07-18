//! Engine-neutral OAuth 2.1 resource-server primitives.
//!
//! MCP requires a Streamable HTTP server to act as an OAuth 2.1 protected
//! resource: advertise its Protected Resource Metadata document
//! ([RFC 9728](https://www.rfc-editor.org/rfc/rfc9728)) and answer
//! unauthorized requests with a `WWW-Authenticate` bearer challenge that
//! points at it. This module owns the engine-neutral half of that
//! contract — options, one-time resolution into a ready-to-serve document,
//! and the challenge — so any [`HttpEngine`](super::engine::HttpEngine)
//! (Volga, axum, hyper, a custom adapter) serves byte-identical metadata.
//!
//! Protocol-level types come from
//! [`volga-oauth-core`](https://docs.rs/volga-oauth-core), which carries
//! no HTTP I/O and no dependency on the Volga framework; the relevant
//! ones are re-exported from [`crate::auth::oauth`].
//!
//! Token *validation* deliberately stays the engine's job (the default
//! Volga adapter uses Volga's bearer/JWKS pipeline; a custom engine brings
//! its own middleware) — see the authorization contract on
//! [`HttpEngine`](super::engine::HttpEngine).

use bytes::Bytes;
use std::sync::Arc;

use crate::error::{Error, ErrorCode};

pub use volga_oauth_core::{
    BearerChallenge, OAuthError, OAuthErrorCode, ProtectedResourceMetadata,
    WELL_KNOWN_PROTECTED_RESOURCE, canonicalize_resource_uri, protected_resource_metadata_url,
};

/// Engine-neutral OAuth protected-resource options.
///
/// Configured with
/// [`HttpServer::with_oauth_metadata`](crate::transport::http::HttpServer::with_oauth_metadata)
/// and resolved once at server start into a ready-to-serve document.
/// The `resource` identifier defaults to the server's own URL
/// (`proto://addr/endpoint`) and only needs
/// [`with_resource`](Self::with_resource) when the public URL differs
/// from the bind address — e.g. behind a reverse proxy.
///
/// # Example
/// ```no_run
/// use neva::App;
///
/// let app = App::new()
///     .with_options(|opt| opt
///         .with_http(|http| http
///             .with_oauth_metadata(|oauth| oauth
///                 .with_authorization_servers(["https://auth.example.com"])
///                 .with_scopes(["mcp:tools"]))
///         )
///     );
/// ```
#[derive(Debug, Clone, Default)]
pub struct OAuthResourceOptions {
    metadata: ProtectedResourceMetadata,
}

impl OAuthResourceOptions {
    /// Overrides the canonical resource identifier URI ([RFC 8707](https://www.rfc-editor.org/rfc/rfc8707)).
    ///
    /// Defaults to the server's own URL. Set it when clients reach the
    /// server through a different public URL than the bind address
    /// (reverse proxy, TLS termination). The value is canonicalized —
    /// scheme/host lowercased, default ports dropped.
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// let app = App::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_oauth_metadata(|oauth| oauth
    ///                 .with_resource("https://api.example.com/mcp"))
    ///         )
    ///     );
    /// ```
    pub fn with_resource(mut self, uri: impl Into<String>) -> Self {
        self.metadata.resource = uri.into();
        self
    }

    /// Sets the issuer identifiers of the authorization servers that can
    /// authorize access to this MCP server.
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// let app = App::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_oauth_metadata(|oauth| oauth
    ///                 .with_authorization_servers(["https://auth.example.com"]))
    ///         )
    ///     );
    /// ```
    pub fn with_authorization_servers<I, S>(mut self, servers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.metadata = self.metadata.with_authorization_servers(servers);
        self
    }

    /// Sets the scope values clients should request to access this server.
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// let app = App::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_oauth_metadata(|oauth| oauth
    ///                 .with_scopes(["mcp:tools", "mcp:resources"]))
    ///         )
    ///     );
    /// ```
    pub fn with_scopes<I, S>(mut self, scopes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.metadata = self.metadata.with_scopes(scopes);
        self
    }

    /// Full-document escape hatch: configures any remaining
    /// [RFC 9728](https://www.rfc-editor.org/rfc/rfc9728) field on the
    /// underlying [`ProtectedResourceMetadata`] builder.
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// let app = App::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_oauth_metadata(|oauth| oauth
    ///                 .with_metadata(|md| md.with_resource_name("Weather MCP")))
    ///         )
    ///     );
    /// ```
    pub fn with_metadata<F>(mut self, config: F) -> Self
    where
        F: FnOnce(ProtectedResourceMetadata) -> ProtectedResourceMetadata,
    {
        self.metadata = config(self.metadata);
        self
    }

    /// Resolves the options against the server's base URL into the
    /// ready-to-serve [`OAuthResource`]: canonicalizes the resource
    /// identifier (defaulting it to `base_url`), derives the metadata
    /// URL/path per RFC 9728 §3.1, pre-serializes the document, and
    /// pre-renders the `WWW-Authenticate` challenge.
    pub(crate) fn resolve(self, base_url: &str) -> Result<OAuthResource, Error> {
        let mut metadata = self.metadata;
        let resource = if metadata.resource.is_empty() {
            base_url
        } else {
            metadata.resource.as_str()
        };
        let resource = canonicalize_resource_uri(resource).map_err(config_error)?;
        let metadata_url = protected_resource_metadata_url(&resource).map_err(config_error)?;
        let challenge = BearerChallenge::new()
            .with_resource_metadata(metadata_url.as_str())
            .to_string();
        metadata.resource = resource;

        Ok(OAuthResource {
            body: serde_json::to_vec(&metadata).map_err(Error::from)?.into(),
            metadata_path: well_known_path(&metadata_url).into(),
            metadata_url: metadata_url.into(),
            challenge: challenge.into(),
            resource: metadata.resource.into(),
        })
    }
}

/// Resolved, ready-to-serve protected-resource state.
///
/// Built once by the transport at server start and handed to the engine
/// through [`HttpContext`](super::context::HttpContext); request-time
/// handlers only clone cheap `Bytes` / `Arc<str>` handles.
#[derive(Debug, Clone)]
pub(crate) struct OAuthResource {
    /// Pre-serialized RFC 9728 metadata document.
    pub(crate) body: Bytes,
    /// Absolute URL of the metadata document — the `resource_metadata`
    /// value of the bearer challenge.
    pub(crate) metadata_url: Arc<str>,
    /// Path the engine mounts the metadata route on
    /// (e.g. `/.well-known/oauth-protected-resource/mcp`).
    pub(crate) metadata_path: Arc<str>,
    /// Pre-rendered `WWW-Authenticate` header value.
    pub(crate) challenge: Arc<str>,
    /// Canonicalized resource identifier (RFC 8707) — the audience value
    /// access tokens must be bound to.
    pub(crate) resource: Arc<str>,
}

/// Strips the scheme and authority off an absolute URL, leaving the path.
///
/// `metadata_url` always carries a path (RFC 9728 §3.1 inserts the
/// well-known segment), so the fallback is unreachable — it only exists
/// to keep the function total without panicking.
fn well_known_path(url: &str) -> &str {
    url.find("://")
        .map(|scheme| &url[scheme + 3..])
        .and_then(|rest| rest.find('/').map(|path| &rest[path..]))
        .unwrap_or(WELL_KNOWN_PROTECTED_RESOURCE)
}

/// Maps a startup-time OAuth configuration failure onto neva's error type.
fn config_error(err: OAuthError) -> Error {
    Error::new(ErrorCode::InternalError, err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_derives_resource_from_base_url() {
        let resource = OAuthResourceOptions::default()
            .resolve("http://127.0.0.1:3000/mcp")
            .unwrap();

        assert_eq!(
            &*resource.metadata_url,
            "http://127.0.0.1:3000/.well-known/oauth-protected-resource/mcp"
        );
        assert_eq!(
            &*resource.metadata_path,
            "/.well-known/oauth-protected-resource/mcp"
        );

        let doc: ProtectedResourceMetadata = serde_json::from_slice(&resource.body).unwrap();
        assert_eq!(doc.resource, "http://127.0.0.1:3000/mcp");
    }

    #[test]
    fn it_canonicalizes_resource_override() {
        let resource = OAuthResourceOptions::default()
            .with_resource("HTTPS://API.Example.COM:443/mcp")
            .resolve("http://127.0.0.1:3000/mcp")
            .unwrap();

        let doc: ProtectedResourceMetadata = serde_json::from_slice(&resource.body).unwrap();
        assert_eq!(doc.resource, "https://api.example.com/mcp");
        assert_eq!(
            &*resource.metadata_url,
            "https://api.example.com/.well-known/oauth-protected-resource/mcp"
        );
    }

    #[test]
    fn it_rejects_invalid_resource() {
        let err = OAuthResourceOptions::default()
            .with_resource("not a uri")
            .resolve("http://127.0.0.1:3000/mcp")
            .unwrap_err();

        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn it_serializes_authorization_servers_and_scopes() {
        let resource = OAuthResourceOptions::default()
            .with_authorization_servers(["https://auth.example.com"])
            .with_scopes(["mcp:tools"])
            .resolve("http://127.0.0.1:3000/mcp")
            .unwrap();

        let doc: ProtectedResourceMetadata = serde_json::from_slice(&resource.body).unwrap();
        assert_eq!(doc.authorization_servers, ["https://auth.example.com"]);
        assert_eq!(doc.scopes_supported, ["mcp:tools"]);
    }

    #[test]
    fn it_passes_through_full_metadata_fields() {
        let resource = OAuthResourceOptions::default()
            .with_metadata(|md| md.with_resource_name("Weather MCP"))
            .resolve("http://127.0.0.1:3000/mcp")
            .unwrap();

        let doc: ProtectedResourceMetadata = serde_json::from_slice(&resource.body).unwrap();
        assert_eq!(doc.resource_name.as_deref(), Some("Weather MCP"));
    }

    #[test]
    fn it_prerenders_a_parseable_challenge() {
        let resource = OAuthResourceOptions::default()
            .resolve("http://127.0.0.1:3000/mcp")
            .unwrap();

        let challenge = BearerChallenge::parse(&resource.challenge).unwrap();
        assert_eq!(
            challenge.resource_metadata(),
            Some("http://127.0.0.1:3000/.well-known/oauth-protected-resource/mcp")
        );
    }

    #[test]
    fn it_mounts_at_well_known_root_for_root_endpoint() {
        let resource = OAuthResourceOptions::default()
            .resolve("http://127.0.0.1:3000/")
            .unwrap();

        assert_eq!(&*resource.metadata_path, WELL_KNOWN_PROTECTED_RESOURCE);
    }
}

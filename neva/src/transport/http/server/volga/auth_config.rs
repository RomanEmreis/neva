//! Authentication and Authorization configuration tools

use crate::transport::http::core::types::DefaultClaims;
use std::fmt::Debug;
#[cfg(feature = "server-oauth")]
use volga::auth::OAuthConfig as VolgaOAuthConfig;
use volga::auth::{Algorithm, AuthClaims, Authorizer, BearerAuthConfig, DecodingKey, predicate};

// Bridge Volga's `AuthClaims` onto neva's canonical, engine-agnostic
// `DefaultClaims`. The type itself lives in `core::types` and already
// implements neva's neutral `Claims` trait, so the same struct flows
// through every engine's request pipeline; this impl is what lets it
// also feed Volga's bearer-auth pipeline when the Volga adapter is on.
impl AuthClaims for DefaultClaims {
    #[inline]
    fn role(&self) -> Option<&str> {
        self.role.as_deref()
    }

    #[inline]
    fn roles(&self) -> Option<&[String]> {
        self.roles.as_deref()
    }

    #[inline]
    fn permissions(&self) -> Option<&[String]> {
        self.permissions.as_deref()
    }
}

/// Represents authentication and authorization configuration
pub struct AuthConfig<C: AuthClaims = DefaultClaims> {
    inner: BearerAuthConfig,

    authorizer: Authorizer<C>,

    /// Whether the user configured an audience themselves (via
    /// [`with_aud`](Self::with_aud) / [`with_resource`](Self::with_resource) /
    /// [`with_resources`](Self::with_resources)) -- suppresses the
    /// OAuth-mode default of binding tokens to the canonical resource URI.
    #[cfg(feature = "server-oauth")]
    aud_configured: bool,

    /// Whether the user configured acceptable issuers themselves --
    /// suppresses the OAuth-mode default of requiring the configured
    /// issuer.
    #[cfg(feature = "server-oauth")]
    iss_configured: bool,

    #[cfg(feature = "server-oauth")]
    oauth: Option<OAuthConfig>,
}

impl Debug for AuthConfig {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthConfig { .. }")
    }
}

impl Default for AuthConfig {
    #[inline]
    fn default() -> Self {
        Self {
            inner: BearerAuthConfig::default(),
            authorizer: default_auth_rules(),
            #[cfg(feature = "server-oauth")]
            aud_configured: false,
            #[cfg(feature = "server-oauth")]
            iss_configured: false,
            #[cfg(feature = "server-oauth")]
            oauth: None,
        }
    }
}

impl From<AuthConfig> for BearerAuthConfig {
    #[inline]
    fn from(auth: AuthConfig) -> Self {
        auth.inner
    }
}

impl<C: AuthClaims> AuthConfig<C> {
    /// Specifies an HMAC secret to validate a JWT with (`HS256`, `HS384`,
    /// `HS512`).
    ///
    /// For a public key, such as the one an authorization server signs its
    /// access tokens with, use [`with_decoding_key`](Self::with_decoding_key).
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// let app = App::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_auth(|auth| auth.set_decoding_key(b"a-string-secret-at-least-256-bits-long"))
    ///         )
    ///     );
    /// ```
    pub fn set_decoding_key(self, secret: &[u8]) -> Self {
        self.with_decoding_key(DecodingKey::from_secret(secret))
    }

    /// Specifies the key to validate a JWT with: an HMAC secret, or the
    /// public half of an RSA, EC or Ed25519 key pair, read from PEM or from
    /// a JSON Web Key (`DecodingKey::from_jwk`).
    ///
    /// Pair a public key with [`with_alg`](Self::with_alg): the default
    /// algorithm is `HS256`, and a key of another family than the
    /// configured algorithms fails the server start. Pair it with
    /// [`with_iss`](Self::with_iss) and [`with_aud`](Self::with_aud) too.
    /// This is how a server validates an authorization server's tokens
    /// without reaching the issuer at all. The key does not follow the
    /// issuer's rotations: a re-keyed issuer means a restart with the new
    /// key. Where the issuer's key set can be fetched from some URL, OAuth
    /// issuer mode with `OAuthConfig::with_jwks_uri` tracks rotations
    /// instead.
    ///
    /// A static key and OAuth issuer mode (`with_oauth`) exclude each
    /// other: a server configured with both fails to start.
    ///
    /// # Example
    /// ```no_run
    /// use neva::{App, auth::{Algorithm, DecodingKey}};
    ///
    /// // the authorization server's public key, shipped with the deployment
    /// let key = DecodingKey::from_pem_file("/etc/mcp/issuer.pem");
    ///
    /// let app = App::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_auth(|auth| auth
    ///                 .with_decoding_key(key)
    ///                 .with_alg(Algorithm::ES256)
    ///                 .with_iss(["http://localhost:8787"])
    ///                 .with_aud(["http://localhost:3000/mcp"]))
    ///         )
    ///     );
    /// ```
    pub fn with_decoding_key(mut self, key: DecodingKey) -> Self {
        self.inner = self.inner.set_decoding_key(key);
        self
    }

    /// Specifies the algorithm supported for verifying JWTs
    ///
    /// The first call replaces the default, and later calls add to the
    /// accepted algorithms.
    ///
    /// Default: [`Algorithm::HS256`]
    /// # Example
    /// ```no_run
    /// use neva::{App, auth::Algorithm};
    ///
    /// let app = App::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_auth(|auth| auth.with_alg(Algorithm::RS256))
    ///         )
    ///     );
    /// ```
    pub fn with_alg(mut self, alg: Algorithm) -> Self {
        self.inner = self.inner.with_alg(alg);
        self
    }

    /// Sets one or more acceptable audience members
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// let app = App::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_auth(|auth| auth.with_aud(["some audience"]))
    ///         )
    ///     );
    /// ```
    pub fn with_aud<I, T>(mut self, aud: I) -> Self
    where
        T: ToString,
        I: AsRef<[T]>,
    {
        self.inner = self.inner.with_aud(aud);
        #[cfg(feature = "server-oauth")]
        {
            self.aud_configured = true;
        }
        self
    }

    /// Adds an OAuth 2.0 resource indicator (RFC 8707): the URI joins the
    /// accepted audience set and the `aud` claim becomes required in
    /// tokens.
    ///
    /// In OAuth issuer mode this is set automatically to the server's
    /// canonical resource URI -- call it only to accept a different
    /// audience (e.g. a public URL behind a reverse proxy).
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// let app = App::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_auth(|auth| auth.with_resource("https://api.example.com/mcp"))
    ///         )
    ///     );
    /// ```
    #[cfg(feature = "server-oauth")]
    pub fn with_resource(mut self, uri: impl Into<String>) -> Self {
        self.inner = self.inner.with_resource(uri);
        self.aud_configured = true;
        self
    }

    /// Adds multiple OAuth 2.0 resource indicators (RFC 8707). See
    /// [`with_resource`](Self::with_resource).
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// let app = App::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_auth(|auth| auth
    ///                 .with_resources(["https://api.example.com/mcp"]))
    ///         )
    ///     );
    /// ```
    #[cfg(feature = "server-oauth")]
    pub fn with_resources<I, U>(mut self, uris: I) -> Self
    where
        I: IntoIterator<Item = U>,
        U: Into<String>,
    {
        self.inner = self.inner.with_resources(uris);
        self.aud_configured = true;
        self
    }

    /// Sets one or more acceptable issuers: a token must carry one of them
    /// in `iss`, and a token without `iss` is refused.
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// let app = App::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_auth(|auth| auth.with_iss(["some issuer"]))
    ///         )
    ///     );
    /// ```
    pub fn with_iss<I, T>(mut self, iss: I) -> Self
    where
        T: ToString,
        I: AsRef<[T]>,
    {
        self.inner = self.inner.with_iss(iss);
        #[cfg(feature = "server-oauth")]
        {
            self.iss_configured = true;
        }
        self
    }

    /// Specifies whether to validate the `aud` field or not.
    ///
    /// It will return an error if the aud field is not a member of the audience provided.
    /// Validation only happens if the aud claim is present in the token.
    ///
    /// Default: `true`
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// let app = App::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_auth(|auth| auth.validate_aud(true))
    ///         )
    ///     );
    /// ```
    pub fn validate_aud(mut self, validate: bool) -> Self {
        self.inner = self.inner.validate_aud(validate);
        self
    }

    /// Specifies whether to validate the `exp` field or not.
    ///
    /// It will return an error if the time in the `exp` field is past.
    ///
    /// Default: `true`
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// let app = App::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_auth(|auth| auth.validate_exp(true))
    ///         )
    ///     );
    /// ```
    pub fn validate_exp(mut self, validate: bool) -> Self {
        self.inner = self.inner.validate_exp(validate);
        self
    }

    /// Specifies whether to validate the `nbf` field or not.
    ///
    /// It will return an error if the current timestamp is before the time in the `nbf` field.
    /// Validation only happens if the `nbf` claim is present in the token.
    ///
    /// Default: `false`
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// let app = App::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_auth(|auth| auth.validate_nbf(true))
    ///         )
    ///     );
    /// ```
    pub fn validate_nbf(mut self, validate: bool) -> Self {
        self.inner = self.inner.validate_nbf(validate);
        self
    }

    /// Specifies whether the MCP endpoint accepts requests over HTTPS only.
    ///
    /// When `true`, a request over plain HTTP from a peer that is not
    /// loopback is refused with `400 Bad Request` and
    /// `WWW-Authenticate: Bearer error="invalid_request"`. A server that
    /// terminates TLS itself (`with_tls`) is never refused.
    ///
    /// Turn it off only where TLS ends in front of the server, or where the
    /// network in between is trusted: a reverse proxy or an ingress that
    /// terminates TLS, a service mesh sidecar, a container whose published
    /// port reaches the server from the bridge gateway rather than from
    /// loopback. The server does not read `X-Forwarded-Proto`, so with the
    /// check off it takes a token from any peer that reaches it. Never turn
    /// it off on a server reachable directly over plain HTTP.
    ///
    /// This is not the `require_https` of the client configuration in
    /// `OAuthConfig::with_config`: that one governs the URLs the server
    /// fetches the issuer's keys from.
    ///
    /// Default: `true`
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// let app = App::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .bind("0.0.0.0:3000")
    ///             .with_auth(|auth| auth
    ///                 .set_decoding_key(b"a-string-secret-at-least-256-bits-long")
    ///                 // TLS ends at the ingress
    ///                 .require_https(false))
    ///         )
    ///     );
    /// ```
    pub fn require_https(mut self, required: bool) -> Self {
        self.inner = self.inner.require_https(required);
        self
    }

    /// Switches token validation to OAuth 2.1/OIDC issuer mode: instead
    /// of a static decoding key, the issuer's JSON Web Key Set is
    /// discovered (RFC 8414, with an OIDC fallback), or fetched from
    /// [`OAuthConfig::with_jwks_uri`], and used to validate incoming JWTs,
    /// keyed by each token's `kid` -- with rotation, refresh cooldown and
    /// key-age limits handled by Volga.
    ///
    /// MCP defaults applied automatically unless overridden: the token's
    /// `aud` must contain the server's canonical resource URI (RFC 8707;
    /// override via [`with_aud`](Self::with_aud) /
    /// [`with_resource`](Self::with_resource)) and its `iss` must match
    /// the configured issuer (override via [`with_iss`](Self::with_iss)).
    /// The Protected Resource Metadata document is derived from the
    /// issuer too, when not configured explicitly.
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// let app = App::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_auth(|auth| auth
    ///                 .with_oauth(|oauth| oauth.with_issuer("https://auth.example.com")))
    ///         )
    ///     );
    /// ```
    #[cfg(feature = "server-oauth")]
    pub fn with_oauth<F>(mut self, config: F) -> Self
    where
        F: FnOnce(OAuthConfig) -> OAuthConfig,
    {
        self.oauth = Some(config(OAuthConfig::default()));
        self
    }

    /// The issuer configured through [`with_oauth`](Self::with_oauth),
    /// if any -- used by the transport to seed the Protected Resource
    /// Metadata document when it was not configured explicitly.
    #[cfg(feature = "server-oauth")]
    pub(crate) fn oauth_issuer(&self) -> Option<&str> {
        self.oauth.as_ref().and_then(|o| o.issuer.as_deref())
    }

    /// Applies the MCP token-validation defaults for OAuth issuer mode:
    /// audience = the canonical resource URI (RFC 8707, `aud` becomes
    /// required) and issuer = the configured issuer. No-ops for anything
    /// the user configured explicitly, and entirely outside OAuth mode.
    #[cfg(feature = "server-oauth")]
    pub(crate) fn apply_mcp_defaults(&mut self, resource: Option<&str>) {
        let Some(oauth) = &self.oauth else {
            return;
        };
        if !self.iss_configured
            && let Some(issuer) = oauth.issuer.as_deref()
        {
            let issuer = issuer.to_owned();
            self.inner = std::mem::take(&mut self.inner).with_iss([issuer]);
        }
        if !self.aud_configured
            && let Some(resource) = resource
        {
            self.inner = std::mem::take(&mut self.inner).with_resource(resource);
        }
    }

    /// Takes the Volga-facing OAuth issuer configuration out, validating
    /// it: `with_oauth` without `with_issuer`, or next to a static decoding
    /// key, is a configuration error that must fail startup.
    #[cfg(feature = "server-oauth")]
    pub(crate) fn take_oauth(&mut self) -> Result<Option<VolgaOAuthConfig>, crate::error::Error> {
        use crate::error::{Error, ErrorCode};

        let Some(oauth) = self.oauth.take() else {
            return Ok(None);
        };
        if self.inner.decoding_key().is_some() {
            return Err(Error::new(
                ErrorCode::InternalError,
                "Bearer auth is ambiguous: both a static decoding key \
                 (`with_decoding_key` / `set_decoding_key`) and an OAuth issuer \
                 (`with_oauth`) are configured; pick one",
            ));
        }
        if oauth.issuer.is_none() {
            return Err(Error::new(
                ErrorCode::InternalError,
                "OAuth issuer is not configured; call `with_oauth(|oauth| oauth.with_issuer(..))`",
            ));
        }
        Ok(Some(oauth.inner))
    }

    /// Deconstructs into [`Authorizer`] and [`BearerAuthConfig`]
    pub(crate) fn into_parts(self) -> (BearerAuthConfig, Authorizer<C>) {
        (self.inner, self.authorizer)
    }
}

/// Configuration of the OAuth 2.1/OIDC issuer whose keys validate bearer
/// tokens, used with [`AuthConfig::with_oauth`].
///
/// A thin neva wrapper over [`volga::auth::OAuthConfig`] that records the
/// issuer, so the transport can derive the MCP defaults from it (required
/// `iss`, the Protected Resource Metadata document). Everything else has
/// production-safe defaults; reach the remaining Volga knobs through
/// [`with_config`](Self::with_config).
#[cfg(feature = "server-oauth")]
#[derive(Default)]
pub struct OAuthConfig {
    issuer: Option<String>,
    inner: VolgaOAuthConfig,
}

#[cfg(feature = "server-oauth")]
impl Debug for OAuthConfig {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthConfig")
            .field("issuer", &self.issuer)
            .finish()
    }
}

#[cfg(feature = "server-oauth")]
impl OAuthConfig {
    /// Sets the issuer identifier URL whose keys validate bearer tokens.
    /// Mandatory -- OAuth mode without an issuer fails server start.
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// let app = App::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_auth(|auth| auth
    ///                 .with_oauth(|oauth| oauth.with_issuer("https://auth.example.com")))
    ///         )
    ///     );
    /// ```
    pub fn with_issuer(mut self, issuer: impl Into<String>) -> Self {
        let issuer = issuer.into();
        self.inner = self.inner.with_issuer(issuer.as_str());
        self.issuer = Some(issuer);
        self
    }

    /// Fetches the issuer's JSON Web Key Set from `url` instead of
    /// discovering it from the issuer's metadata.
    ///
    /// For a server that cannot reach the URL its tokens name in `iss`:
    /// the issuer runs on the host as `http://localhost:8787`, and the
    /// server, in a container, reaches it as
    /// `http://host.docker.internal:8787`; or a Kubernetes service sits
    /// behind a public ingress host. Discovery is skipped, since the
    /// issuer's metadata names the unreachable host twice: as its `issuer`,
    /// which must equal the URL it was fetched from, and in its `jwks_uri`.
    ///
    /// [`with_issuer`](Self::with_issuer) is still required and keeps its
    /// meaning: tokens must carry it in `iss`, and the Protected Resource
    /// Metadata document sends clients to it. Only where the keys come from
    /// changes; rotation, the refresh cooldown and the key age work as they
    /// do with discovery. The transport policy set through
    /// [`with_config`](Self::with_config) applies to `url` as well, so a
    /// plain `http` URL needs `require_https(false)`.
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// let app = App::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_auth(|auth| auth.with_oauth(|oauth| oauth
    ///                 // what tokens carry in `iss`, and where clients go
    ///                 .with_issuer("http://localhost:8787")
    ///                 // where this server, in a container, gets the keys
    ///                 .with_jwks_uri("http://host.docker.internal:8787/jwks")
    ///                 .with_config(|cfg| cfg.with_client_config(|c| c.require_https(false)))))
    ///         )
    ///     );
    /// ```
    pub fn with_jwks_uri(mut self, url: impl Into<String>) -> Self {
        self.inner = self.inner.with_jwks_uri(url);
        self
    }

    /// Sets the minimum interval between two JWKS refresh attempts.
    ///
    /// Default: 60 seconds.
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    /// use std::time::Duration;
    ///
    /// let app = App::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_auth(|auth| auth.with_oauth(|oauth| oauth
    ///                 .with_issuer("https://auth.example.com")
    ///                 .with_refresh_cooldown(Duration::from_secs(120))))
    ///         )
    ///     );
    /// ```
    pub fn with_refresh_cooldown(mut self, cooldown: std::time::Duration) -> Self {
        self.inner = self.inner.with_refresh_cooldown(cooldown);
        self
    }

    /// Sets the age after which the cached key set is re-fetched even for
    /// known key ids, so revoked keys do not stay trusted indefinitely.
    ///
    /// Default: 15 minutes.
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    /// use std::time::Duration;
    ///
    /// let app = App::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_auth(|auth| auth.with_oauth(|oauth| oauth
    ///                 .with_issuer("https://auth.example.com")
    ///                 .with_max_key_age(Duration::from_secs(300))))
    ///         )
    ///     );
    /// ```
    pub fn with_max_key_age(mut self, max_age: std::time::Duration) -> Self {
        self.inner = self.inner.with_max_key_age(max_age);
        self
    }

    /// Escape hatch to the underlying [`volga::auth::OAuthConfig`] for
    /// knobs without a neva-level counterpart (e.g. the HTTP client policy
    /// for discovery and key fetches).
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// let app = App::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_auth(|auth| auth.with_oauth(|oauth| oauth
    ///                 .with_issuer("http://127.0.0.1:5000")
    ///                 // local dev issuer over plain http
    ///                 .with_config(|cfg| cfg.with_client_config(|c| c.require_https(false)))))
    ///         )
    ///     );
    /// ```
    pub fn with_config<F>(mut self, config: F) -> Self
    where
        F: FnOnce(VolgaOAuthConfig) -> VolgaOAuthConfig,
    {
        self.inner = config(self.inner);
        self
    }
}

/// Creates default authorization and authentication rules
#[inline]
pub(super) fn default_auth_rules() -> Authorizer<DefaultClaims> {
    predicate(|_| true)
}

#[cfg(all(test, feature = "server-oauth"))]
mod tests {
    use super::*;

    #[test]
    fn it_records_the_issuer() {
        let auth = AuthConfig::default().with_oauth(|o| o.with_issuer("https://auth.example.com"));
        assert_eq!(auth.oauth_issuer(), Some("https://auth.example.com"));
    }

    #[test]
    fn take_oauth_without_oauth_mode_is_none() {
        let mut auth = AuthConfig::default();
        assert!(auth.take_oauth().unwrap().is_none());
    }

    #[test]
    fn take_oauth_without_issuer_fails() {
        let mut auth = AuthConfig::default().with_oauth(|o| o);
        assert!(auth.take_oauth().is_err());
    }

    #[test]
    fn take_oauth_with_issuer_yields_volga_config() {
        let mut auth =
            AuthConfig::default().with_oauth(|o| o.with_issuer("https://auth.example.com"));
        assert!(auth.take_oauth().unwrap().is_some());
    }

    #[test]
    fn a_static_key_next_to_oauth_fails() {
        let mut auth = AuthConfig::default()
            .with_decoding_key(DecodingKey::from_secret(b"secret"))
            .with_oauth(|o| o.with_issuer("https://auth.example.com"));
        let err = auth.take_oauth().unwrap_err();
        assert!(err.to_string().contains("ambiguous"), "{err}");

        // the HMAC shorthand is the same static key
        let mut auth = AuthConfig::default()
            .with_oauth(|o| o.with_issuer("https://auth.example.com"))
            .set_decoding_key(b"secret");
        assert!(auth.take_oauth().is_err());
    }

    #[test]
    fn a_static_key_alone_is_not_oauth_mode() {
        let mut auth = AuthConfig::default().with_decoding_key(DecodingKey::from_secret(b"secret"));
        assert!(auth.take_oauth().unwrap().is_none());
    }

    #[test]
    fn it_forwards_the_jwks_uri() {
        let mut auth = AuthConfig::default().with_oauth(|o| {
            o.with_issuer("http://localhost:8787")
                .with_jwks_uri("http://host.docker.internal:8787/jwks")
        });
        // the issuer stays the public identifier
        assert_eq!(auth.oauth_issuer(), Some("http://localhost:8787"));

        let volga = auth.take_oauth().unwrap().unwrap();
        assert!(
            format!("{volga:?}").contains("Some(\"http://host.docker.internal:8787/jwks\")"),
            "{volga:?}"
        );
    }

    // `BearerAuthConfig` keeps its audience set private; its `Debug` impl
    // exposes the RFC 8707 `resources` list, which `with_resource` feeds --
    // assert through that.
    fn resources_of(auth: &AuthConfig) -> String {
        format!("{:?}", auth.inner)
    }

    #[test]
    fn mcp_defaults_bind_audience_to_the_resource() {
        let mut auth =
            AuthConfig::default().with_oauth(|o| o.with_issuer("https://auth.example.com"));
        auth.apply_mcp_defaults(Some("http://127.0.0.1:3000/mcp"));
        assert!(resources_of(&auth).contains("http://127.0.0.1:3000/mcp"));
    }

    #[test]
    fn explicit_audience_suppresses_the_default() {
        let mut auth = AuthConfig::default()
            .with_aud(["my-audience"])
            .with_oauth(|o| o.with_issuer("https://auth.example.com"));
        auth.apply_mcp_defaults(Some("http://127.0.0.1:3000/mcp"));
        assert!(!resources_of(&auth).contains("http://127.0.0.1:3000/mcp"));
    }

    #[test]
    fn mcp_defaults_outside_oauth_mode_are_noop() {
        let mut auth = AuthConfig::default();
        auth.apply_mcp_defaults(Some("http://127.0.0.1:3000/mcp"));
        assert!(!resources_of(&auth).contains("http://127.0.0.1:3000/mcp"));
    }

    // The HTTPS switch is private to `BearerAuthConfig` as well, and shows in
    // the same `Debug` output.
    #[test]
    fn https_is_required_by_default() {
        let auth = AuthConfig::default();
        assert!(format!("{:?}", auth.inner).contains("require_https: true"));
    }

    #[test]
    fn it_forwards_require_https() {
        let auth = AuthConfig::default().require_https(false);
        assert!(format!("{:?}", auth.inner).contains("require_https: false"));
    }
}

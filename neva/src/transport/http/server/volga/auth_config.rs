//! Authentication and Authorization configuration tools

use crate::transport::http::core::types::DefaultClaims;
use std::fmt::Debug;
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
    /// Specifies a security key to validate a JWT from a secret
    pub fn set_decoding_key(mut self, secret: &[u8]) -> Self {
        self.inner = self
            .inner
            .set_decoding_key(DecodingKey::from_secret(secret));
        self
    }

    /// Specifies the algorithm supported for verifying JWTs
    ///
    /// Default: [`Algorithm::HS256`]
    /// # Example
    /// ```no_run
    /// use neva::{App, auth::Algorithm};
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
        self
    }

    /// Sets one or more acceptable issuers
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
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

    /// Deconstructs into [`Authorizer`] and [`BearerAuthConfig`]
    pub(crate) fn into_parts(self) -> (BearerAuthConfig, Authorizer<C>) {
        (self.inner, self.authorizer)
    }
}

/// Creates default authorization and authentication rules
#[inline]
pub(super) fn default_auth_rules() -> Authorizer<DefaultClaims> {
    predicate(|_| true)
}

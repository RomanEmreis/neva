//! The grant that presents a JWT somebody else issued.
//!
//! [`AssertionProvider`] is the seam for RFC 7523 section 2.1: the client hands
//! the authorization server a JWT it did not mint, and the server decides what
//! that identity is worth. Where the JWT comes from is deliberately outside the
//! protocol -- a workload platform writes one to a file, a SPIFFE agent rotates
//! one, an enterprise IdP issues one at single sign-on -- so this is a trait
//! rather than a string.
//!
//! [`IdentityAssertion`] is the one implementation neva ships, for the
//! enterprise-managed authorization profile: it trades an ID token at the
//! identity provider for the cross-domain grant the resource's authorization
//! server accepts (RFC 8693, then RFC 7523).

use super::*;

use volga_oauth_client::token_type;

/// What the flow knows about the token it is asking for, handed to an
/// [`AssertionProvider`] so the assertion can be minted for this server
/// rather than for anything.
///
/// Both identifiers are discovered, not configured: `issuer` is what the
/// resource's Protected Resource Metadata pointed at, and `resource` is the
/// identifier that metadata declares for itself.
///
/// # Examples
/// ```no_run
/// use neva::auth::oauth::AssertionRequest;
///
/// fn audience(request: &AssertionRequest) -> &str {
///     // the grant is minted for the authorization server, and names the
///     // resource it is meant to buy a token for
///     &request.issuer
/// }
/// ```
#[derive(Debug, Clone)]
pub struct AssertionRequest {
    /// The `issuer` identifier of the authorization server the assertion
    /// will be presented to.
    pub issuer: String,

    /// The RFC 9728 resource identifier of the MCP server a token is
    /// wanted for -- the RFC 8707 resource indicator of the grant.
    pub resource: String,

    /// The scopes the flow is about to request.
    pub scopes: Vec<String>,
}

/// Supplies the JWT presented as an RFC 7523 section 2.1 authorization
/// grant, configured with
/// [`with_jwt_bearer`](OAuthClientConfig::with_jwt_bearer).
///
/// Called once per token request, so an implementation reading a rotating
/// credential -- a projected service account token, a SPIFFE SVID -- reads it
/// fresh each time rather than caching what the platform already rotated away.
///
/// The method returns a [`BoxFuture`] rather than being an `async fn`: the
/// provider is held behind `Arc<dyn AssertionProvider>`, and `async fn` in a
/// trait is not dyn-compatible. `Box::pin(async move { ... })` is all an
/// implementation needs, and the alias is neva's own, so implementing this
/// trait pulls in no `futures` dependency.
///
/// A `String` implements it, which covers the credential that does not
/// change for the lifetime of the process.
///
/// # Examples
/// ```no_run
/// use neva::auth::oauth::{AssertionProvider, AssertionRequest};
/// use neva::error::{Error, ErrorCode};
/// use neva::shared::BoxFuture;
///
/// /// Reads the workload JWT its platform projects into the container.
/// struct ProjectedToken(std::path::PathBuf);
///
/// impl AssertionProvider for ProjectedToken {
///     fn assertion(&self, _request: AssertionRequest) -> BoxFuture<'_, Result<String, Error>> {
///         Box::pin(async move {
///             tokio::fs::read_to_string(&self.0)
///                 .await
///                 .map(|jwt| jwt.trim().to_owned())
///                 .map_err(|err| Error::new(ErrorCode::InternalError, err.to_string()))
///         })
///     }
/// }
/// ```
pub trait AssertionProvider: Send + Sync + 'static {
    /// Returns the JWT to send as the `assertion` parameter of the
    /// JWT-bearer grant.
    fn assertion(&self, request: AssertionRequest) -> BoxFuture<'_, Result<String, Error>>;
}

/// A credential that does not change while the process runs -- the whole of
/// what a fixture, a test, or a workload with a long-lived token needs.
impl AssertionProvider for String {
    fn assertion(&self, _request: AssertionRequest) -> BoxFuture<'_, Result<String, Error>> {
        Box::pin(async move { Ok(self.clone()) })
    }
}

/// The enterprise-managed authorization profile: an identity assertion the
/// enterprise identity provider issued at single sign-on, traded there for
/// the cross-domain grant the MCP server's authorization server accepts.
///
/// Two requests stand behind one [`AssertionProvider`]. The first is an
/// RFC 8693 token exchange at the identity provider: the ID token goes up as
/// the `subject_token`, and what comes back is an Identity Assertion JWT
/// Authorization Grant (ID-JAG) audienced to the resource's authorization
/// server and naming the MCP server in its `resource` claim. The second is
/// the flow's own JWT-bearer request, which presents that grant -- so it is
/// the session that makes it, and this type only produces what it sends.
///
/// The `audience` of the exchange is the authorization server's `issuer`
/// identifier and the `resource` is the MCP server's own identifier, both
/// taken from the [`AssertionRequest`] rather than configured: they are what
/// discovery found, and a value written by hand would be a second, staler
/// copy of it.
///
/// Client authentication at the identity provider is separate from client
/// authentication at the resource's authorization server -- two servers, two
/// registrations -- so the credentials here are this type's own, and the ones
/// on [`OAuthClientConfig`] stay with the JWT-bearer request.
///
/// # Examples
/// ```no_run
/// use neva::Client;
/// use neva::auth::oauth::IdentityAssertion;
///
/// # fn run(id_token: String) {
/// let mut client = Client::new()
///     .with_options(|opt| opt
///         .with_http(|http| http
///             .with_oauth(|oauth| oauth
///                 // as registered with the MCP server's authorization server
///                 .with_client_id("mcp-app")
///                 .with_client_secret("s3cret")
///                 // ...and, separately, with the enterprise IdP
///                 .with_identity_assertion(
///                     IdentityAssertion::new("https://acme.idp.example", id_token)
///                         .with_client_id("idp-app")))
///         )
///     );
/// # }
/// ```
pub struct IdentityAssertion {
    issuer: String,
    token_endpoint: Option<String>,
    subject_token: String,
    subject_token_type: String,
    client_id: Option<String>,
    client_secret: Option<String>,
    require_https: bool,
}

impl std::fmt::Debug for IdentityAssertion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // the subject token is a live credential for the identity provider
        // and the secret is one for the client; neither has any business
        // reaching a log
        f.debug_struct("IdentityAssertion")
            .field("issuer", &self.issuer)
            .field("token_endpoint", &self.token_endpoint)
            .field("subject_token_type", &self.subject_token_type)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "[redacted]"),
            )
            .field("require_https", &self.require_https)
            .finish_non_exhaustive()
    }
}

impl IdentityAssertion {
    /// Exchanges `subject_token` -- an OpenID Connect ID token from the
    /// enterprise identity provider at `issuer` -- for the grant presented
    /// to the MCP server's authorization server.
    ///
    /// `issuer` is the identity provider's `issuer` identifier, not its
    /// token endpoint: the endpoint is discovered from it, so the two
    /// cannot drift apart.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::auth::oauth::IdentityAssertion;
    ///
    /// # fn run(id_token: String) {
    /// let assertion = IdentityAssertion::new("https://acme.idp.example", id_token);
    /// # }
    /// ```
    pub fn new(issuer: impl Into<String>, subject_token: impl Into<String>) -> Self {
        Self {
            issuer: issuer.into(),
            token_endpoint: None,
            subject_token: subject_token.into(),
            subject_token_type: token_type::ID_TOKEN.to_owned(),
            client_id: None,
            client_secret: None,
            require_https: true,
        }
    }

    /// Names the identity provider's token endpoint outright, instead of
    /// discovering it from the issuer.
    ///
    /// The client signed the user in at this provider, so it already knows
    /// where the endpoint is -- and the profile asks nothing more of the IdP
    /// than that one endpoint. Discovery is the convenience; this is the way
    /// through when the provider publishes no metadata document, or publishes
    /// one that is not a complete RFC 8414 record because it is an identity
    /// provider rather than a resource's authorization server.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::auth::oauth::IdentityAssertion;
    ///
    /// # fn run(id_token: String) {
    /// let assertion = IdentityAssertion::new("https://acme.idp.example", id_token)
    ///     .with_token_endpoint("https://acme.idp.example/oauth2/token");
    /// # }
    /// ```
    pub fn with_token_endpoint(mut self, url: impl Into<String>) -> Self {
        self.token_endpoint = Some(url.into());
        self
    }

    /// Declares what the subject token is, as one of the RFC 8693 section 3
    /// type identifiers. Defaults to an OpenID Connect ID token.
    ///
    /// A client that signed in over SAML exchanges the assertion for a
    /// refresh token first, and presents that here instead.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::auth::oauth::{IdentityAssertion, token_type};
    ///
    /// # fn run(refresh_token: String) {
    /// let assertion = IdentityAssertion::new("https://acme.idp.example", refresh_token)
    ///     .with_subject_token_type(token_type::REFRESH_TOKEN);
    /// # }
    /// ```
    pub fn with_subject_token_type(mut self, token_type: impl Into<String>) -> Self {
        self.subject_token_type = token_type.into();
        self
    }

    /// Identifies this client to the *identity provider* -- the registration
    /// it signed the user in under, which is not the one it holds at the
    /// MCP server's authorization server.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::auth::oauth::IdentityAssertion;
    ///
    /// # fn run(id_token: String) {
    /// let assertion = IdentityAssertion::new("https://acme.idp.example", id_token)
    ///     .with_client_id("idp-app");
    /// # }
    /// ```
    pub fn with_client_id(mut self, client_id: impl Into<String>) -> Self {
        self.client_id = Some(client_id.into());
        self
    }

    /// Authenticates the exchange with a client secret.
    ///
    /// Required exactly when the identity provider required client
    /// authentication for the single sign-on that produced the subject
    /// token: an IdP that authenticated the client then authenticates it
    /// here too.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::auth::oauth::IdentityAssertion;
    ///
    /// # fn run(id_token: String) {
    /// let assertion = IdentityAssertion::new("https://acme.idp.example", id_token)
    ///     .with_client_id("idp-app")
    ///     .with_client_secret("s3cret");
    /// # }
    /// ```
    pub fn with_client_secret(mut self, secret: impl Into<String>) -> Self {
        self.client_secret = Some(secret.into());
        self
    }

    /// Controls whether a plain `http://` identity provider is rejected.
    /// Enabled by default; disable only against a local development IdP.
    ///
    /// Separate from
    /// [`OAuthClientConfig::require_https`](OAuthClientConfig::require_https)
    /// because the identity provider is a separate deployment: a local MCP
    /// server does not make the enterprise IdP local too.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::auth::oauth::IdentityAssertion;
    ///
    /// # fn run(id_token: String) {
    /// let assertion = IdentityAssertion::new("http://localhost:9000", id_token)
    ///     .require_https(false);
    /// # }
    /// ```
    pub fn require_https(mut self, required: bool) -> Self {
        self.require_https = required;
        self
    }

    /// The identity provider's metadata.
    ///
    /// A named token endpoint is taken at its word -- there is nothing else
    /// in the document this profile reads, and the client learned the
    /// endpoint at sign-on.
    ///
    /// Otherwise: RFC 8414 first, then OpenID Connect Discovery when that
    /// path is not served -- the same order and the same `404`-only fallback
    /// `DiscoveryClient::discover_authorization_server` applies to a
    /// resource's authorization server. An enterprise IdP commonly publishes
    /// only the OIDC document, and any failure other than "not here" is the
    /// RFC 8414 location answering, which stands.
    async fn discover(&self) -> Result<AuthorizationServerMetadata, Error> {
        if let Some(endpoint) = &self.token_endpoint {
            // The exchange grant is declared because this record was written
            // here rather than fetched: `AuthorizationServerMetadata::new`
            // starts from the RFC 8414 default of `authorization_code`, and
            // leaving it there would have the client refuse its own request
            // over a capability the provider never got to state.
            return Ok(AuthorizationServerMetadata::new(self.issuer.clone())
                .with_token_endpoint(endpoint.clone())
                .with_grant_types([grant::TOKEN_EXCHANGE]));
        }

        let discovery = DiscoveryClient::with_config(self.client_config());
        match discovery.fetch_server_metadata(&self.issuer).await {
            Err(ClientError::Http(status)) if status.as_u16() == 404 => discovery
                .fetch_oidc_metadata(&self.issuer)
                .await
                .map_err(flow_error),
            other => other.map_err(flow_error),
        }
    }

    fn client_config(&self) -> ClientConfig {
        ClientConfig::new().require_https(self.require_https)
    }
}

impl AssertionProvider for IdentityAssertion {
    fn assertion(&self, request: AssertionRequest) -> BoxFuture<'_, Result<String, Error>> {
        Box::pin(async move {
            let metadata = self.discover().await?;

            let mut client = OAuthClient::new(self.client_id.clone().unwrap_or_default())
                .with_config(self.client_config());

            if let Some(secret) = &self.client_secret {
                client = client.with_secret(secret.clone());
            }

            let exchanged = client
                .exchange_token(&metadata, &self.subject_token, &self.subject_token_type)
                .with_requested_token_type(token_type::ID_JAG)
                // The profile pins both: `audience` is the issuer identifier
                // of the authorization server that will be handed the grant,
                // and `resource` the identifier the MCP server declares for
                // itself. An identity provider evaluates its policy against
                // exactly this pair -- which client, for which server -- so a
                // wrong one is not a wrong parameter but a different question.
                .with_audience(request.issuer)
                .with_resource(request.resource)
                .with_scopes(request.scopes)
                .send()
                .await
                .map_err(flow_error)?;

            // The response says what was issued, and only an ID-JAG is a
            // grant. A server that answered with an access token answered a
            // different request; presenting it as an assertion would buy an
            // `invalid_grant` one round later, with nothing in the message to
            // say the type was wrong.
            if exchanged.issued_token_type != token_type::ID_JAG {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    format!(
                        "`{}` answered the token exchange with `{}` rather than an \
                         identity assertion grant (`{}`)",
                        self.issuer,
                        exchanged.issued_token_type,
                        token_type::ID_JAG
                    ),
                ));
            }

            Ok(exchanged.token)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn it_serves_a_string_assertion_verbatim() {
        let provider = "a.workload.jwt".to_owned();
        let request = AssertionRequest {
            issuer: "https://auth.example.com".into(),
            resource: "https://mcp.example.com/mcp".into(),
            scopes: vec!["mcp:read".into()],
        };

        assert_eq!(provider.assertion(request).await.unwrap(), "a.workload.jwt");
    }

    #[test]
    fn it_defaults_an_identity_assertion_to_an_id_token() {
        let assertion = IdentityAssertion::new("https://idp.example", "id.token");

        assert_eq!(assertion.subject_token_type, token_type::ID_TOKEN);
        assert!(assertion.require_https);
        assert!(assertion.client_id.is_none());
        assert!(assertion.token_endpoint.is_none());
    }

    /// A named endpoint is the whole of what this profile reads from the
    /// provider, so nothing is fetched for it.
    #[tokio::test]
    async fn a_named_token_endpoint_is_taken_at_its_word() {
        let assertion = IdentityAssertion::new("https://idp.example", "id.token")
            .with_token_endpoint("https://idp.example/oauth2/token");

        let metadata = assertion
            .discover()
            .await
            .expect("a named endpoint needs no network");

        assert_eq!(metadata.issuer, "https://idp.example");
        assert_eq!(
            metadata.token_endpoint.as_deref(),
            Some("https://idp.example/oauth2/token")
        );
        assert!(
            metadata
                .grant_types_supported
                .iter()
                .any(|supported| supported == grant::TOKEN_EXCHANGE),
            "a hand-written record has to declare the grant it is written for"
        );
    }

    #[test]
    fn it_overrides_the_subject_token_type() {
        let assertion = IdentityAssertion::new("https://idp.example", "refresh.token")
            .with_subject_token_type(token_type::REFRESH_TOKEN)
            .with_client_id("idp-app")
            .with_client_secret("s3cret")
            .require_https(false);

        assert_eq!(assertion.subject_token_type, token_type::REFRESH_TOKEN);
        assert_eq!(assertion.client_id.as_deref(), Some("idp-app"));
        assert!(!assertion.require_https);
    }

    #[test]
    fn it_keeps_credentials_out_of_debug_output() {
        let assertion =
            IdentityAssertion::new("https://idp.example", "id.token").with_client_secret("s3cret");

        let rendered = format!("{assertion:?}");

        assert!(!rendered.contains("s3cret"));
        assert!(!rendered.contains("id.token"));
        assert!(rendered.contains("[redacted]"));
    }
}

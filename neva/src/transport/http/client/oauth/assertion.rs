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
/// Implement it with a plain `async fn`. The `+ Send` the trait puts on the
/// returned future is what lets the session drive it from a spawned task; an
/// `async fn` whose body holds nothing thread-bound across an `.await`
/// satisfies that without having to say so.
///
/// A `String` implements it, which covers the credential that does not
/// change for the lifetime of the process.
///
/// # Examples
/// ```no_run
/// use neva::auth::oauth::{AssertionProvider, AssertionRequest};
/// use neva::error::{Error, ErrorCode};
///
/// /// Reads the workload JWT its platform projects into the container.
/// struct ProjectedToken(std::path::PathBuf);
///
/// impl AssertionProvider for ProjectedToken {
///     async fn assertion(&self, _request: AssertionRequest) -> Result<String, Error> {
///         tokio::fs::read_to_string(&self.0)
///             .await
///             .map(|jwt| jwt.trim().to_owned())
///             .map_err(|err| Error::new(ErrorCode::InternalError, err.to_string()))
///     }
/// }
/// ```
pub trait AssertionProvider: Send + Sync + 'static {
    /// Returns the JWT to send as the `assertion` parameter of the
    /// JWT-bearer grant.
    fn assertion(
        &self,
        request: AssertionRequest,
    ) -> impl Future<Output = Result<String, Error>> + Send;
}

/// A credential that does not change while the process runs -- the whole of
/// what a fixture, a test, or a workload with a long-lived token needs.
impl AssertionProvider for String {
    async fn assertion(&self, _request: AssertionRequest) -> Result<String, Error> {
        Ok(self.clone())
    }
}

/// The dyn-compatible half of [`AssertionProvider`].
///
/// One configuration holds one provider whose type it does not know, so the
/// session reaches it through `Arc<dyn ..>` -- and a trait method returning
/// `impl Future` cannot be made into one. The boxing that bridges the two
/// lives here rather than in the signature an implementor writes: it costs
/// one allocation per token request, which is the same request that opens a
/// TCP connection.
///
/// Nothing implements this by hand. The blanket impl below covers every
/// [`AssertionProvider`], so it is a detail of the storage rather than a
/// second thing to write.
pub(crate) trait DynAssertionProvider: Send + Sync + 'static {
    /// [`AssertionProvider::assertion`] with its future boxed.
    fn boxed_assertion(&self, request: AssertionRequest) -> BoxFuture<'_, Result<String, Error>>;
}

impl<T: AssertionProvider> DynAssertionProvider for T {
    fn boxed_assertion(&self, request: AssertionRequest) -> BoxFuture<'_, Result<String, Error>> {
        Box::pin(self.assertion(request))
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
///                     IdentityAssertion::new(
///                         "https://acme.idp.example", "idp-app", id_token)))
///         )
///     );
/// # }
/// ```
pub struct IdentityAssertion {
    issuer: String,
    client_id: String,
    token_endpoint: Option<String>,
    subject_token: String,
    subject_token_type: String,
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
            .field("client_id", &self.client_id)
            .field("token_endpoint", &self.token_endpoint)
            .field("subject_token_type", &self.subject_token_type)
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
    /// token endpoint: the endpoint is discovered from it, so the two cannot
    /// drift apart.
    ///
    /// `client_id` is this client's registration at *that* provider, which is
    /// not the one it holds at the MCP server's authorization server. It is
    /// required rather than optional because a client that signed a user in
    /// there has one by construction -- and because it is what the exchange
    /// is identified by: a request carrying no id names no client for the
    /// provider to evaluate its policy against, or to check a
    /// [`with_client_secret`](Self::with_client_secret) against.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::auth::oauth::IdentityAssertion;
    ///
    /// # fn run(id_token: String) {
    /// let assertion =
    ///     IdentityAssertion::new("https://acme.idp.example", "idp-app", id_token);
    /// # }
    /// ```
    pub fn new(
        issuer: impl Into<String>,
        client_id: impl Into<String>,
        subject_token: impl Into<String>,
    ) -> Self {
        Self {
            issuer: issuer.into(),
            client_id: client_id.into(),
            token_endpoint: None,
            subject_token: subject_token.into(),
            subject_token_type: token_type::ID_TOKEN.to_owned(),
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
    /// let assertion = IdentityAssertion::new("https://acme.idp.example", "idp-app", id_token)
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
    /// let assertion =
    ///     IdentityAssertion::new("https://acme.idp.example", "idp-app", refresh_token)
    ///         .with_subject_token_type(token_type::REFRESH_TOKEN);
    /// # }
    /// ```
    pub fn with_subject_token_type(mut self, token_type: impl Into<String>) -> Self {
        self.subject_token_type = token_type.into();
        self
    }

    /// Authenticates the exchange with a client secret.
    ///
    /// Required exactly when the identity provider required client
    /// authentication for the single sign-on that produced the subject
    /// token: an IdP that authenticated the client then authenticates it
    /// here too.
    ///
    /// The registration it is checked against is the `client_id` given to
    /// [`new`](Self::new), which is why that one is not optional.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::auth::oauth::IdentityAssertion;
    ///
    /// # fn run(id_token: String) {
    /// let assertion = IdentityAssertion::new("https://acme.idp.example", "idp-app", id_token)
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
    /// let assertion = IdentityAssertion::new("http://localhost:9000", "idp-app", id_token)
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
    async fn assertion(&self, request: AssertionRequest) -> Result<String, Error> {
        let metadata = self.discover().await?;

        let mut client = OAuthClient::new(self.client_id.clone()).with_config(self.client_config());

        if let Some(secret) = &self.client_secret {
            // The identity provider is a second authorization server with its
            // own registration, so its `token_endpoint_auth_methods_supported`
            // is its own answer too -- and one that advertises only
            // `client_secret_post` refuses the Basic an `OAuthClient` defaults
            // to, over a secret that works in the body. Same negotiation the
            // resource's authorization server gets.
            client = client
                .with_secret(secret.clone())
                .with_auth_method(secret_auth_method(&metadata)?);
        }

        let exchanged = client
            .exchange_token(&metadata, &self.subject_token, &self.subject_token_type)
            .with_requested_token_type(token_type::ID_JAG)
            // The profile pins both: `audience` is the issuer identifier of
            // the authorization server that will be handed the grant, and
            // `resource` the identifier the MCP server declares for itself.
            // An identity provider evaluates its policy against exactly this
            // pair -- which client, for which server -- so a wrong one is not
            // a wrong parameter but a different question.
            .with_audience(request.issuer)
            .with_resource(request.resource)
            .with_scopes(request.scopes)
            .send()
            .await
            .map_err(flow_error)?;

        // The response says what was issued, and only an ID-JAG is a grant. A
        // server that answered with an access token answered a different
        // request; presenting it as an assertion would buy an `invalid_grant`
        // one round later, with nothing in the message to say the type was
        // wrong.
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
        let assertion = IdentityAssertion::new("https://idp.example", "idp-app", "id.token");

        assert_eq!(assertion.subject_token_type, token_type::ID_TOKEN);
        assert_eq!(assertion.client_id, "idp-app");
        assert!(assertion.require_https);
        assert!(assertion.client_secret.is_none());
        assert!(assertion.token_endpoint.is_none());
    }

    /// A named endpoint is the whole of what this profile reads from the
    /// provider, so nothing is fetched for it.
    #[tokio::test]
    async fn a_named_token_endpoint_is_taken_at_its_word() {
        let assertion = IdentityAssertion::new("https://idp.example", "idp-app", "id.token")
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
        let assertion = IdentityAssertion::new("https://idp.example", "idp-app", "refresh.token")
            .with_subject_token_type(token_type::REFRESH_TOKEN)
            .with_client_secret("s3cret")
            .require_https(false);

        assert_eq!(assertion.subject_token_type, token_type::REFRESH_TOKEN);
        assert!(assertion.client_secret.is_some());
        assert!(!assertion.require_https);
    }

    /// An identity provider that publishes RFC 8414 metadata advertising
    /// `auth_methods`, and answers one token exchange with an ID-JAG.
    /// Records every request so a test can assert how the client
    /// authenticated.
    async fn spawn_identity_provider(
        auth_methods: &'static str,
    ) -> (std::net::SocketAddr, Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = seen.clone();

        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 8192];
                let read = stream.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..read]).to_string();
                let root = format!("http://{addr}");
                if let Ok(mut seen) = recorder.lock() {
                    seen.push(request.clone());
                }

                let body = if request.contains("/.well-known/") {
                    format!(
                        r#"{{"issuer":"{root}","token_endpoint":"{root}/token",
                             "grant_types_supported":["urn:ietf:params:oauth:grant-type:token-exchange"],
                             "token_endpoint_auth_methods_supported":[{auth_methods}],
                             "response_types_supported":["code"]}}"#
                    )
                } else {
                    r#"{"access_token":"the.id.jag",
                        "issued_token_type":"urn:ietf:params:oauth:token-type:id-jag",
                        "token_type":"N_A","expires_in":300}"#
                        .to_string()
                };

                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        });
        (addr, seen)
    }

    fn exchange_request() -> AssertionRequest {
        AssertionRequest {
            issuer: "https://auth.example.com".into(),
            resource: "https://mcp.example.com/mcp".into(),
            scopes: vec!["mcp:read".into()],
        }
    }

    /// The identity provider is a second authorization server with its own
    /// registration, so what it accepts at its token endpoint is its own
    /// answer. One that takes only `client_secret_post` refuses the Basic an
    /// `OAuthClient` defaults to -- over a secret that works in the body.
    #[tokio::test]
    async fn it_authenticates_to_the_provider_the_way_the_provider_accepts() {
        let (addr, seen) = spawn_identity_provider(r#""client_secret_post""#).await;

        let assertion = IdentityAssertion::new(format!("http://{addr}"), "idp-app", "id.token")
            .with_client_secret("s3cret")
            .require_https(false);

        assert_eq!(
            assertion.assertion(exchange_request()).await.unwrap(),
            "the.id.jag"
        );

        let requests = seen.lock().unwrap().clone();
        let exchange = requests
            .iter()
            .find(|request| request.contains("POST /token"))
            .expect("the exchange must have reached the provider");

        assert!(exchange.contains("client_id=idp-app"), "{exchange}");
        assert!(exchange.contains("client_secret=s3cret"), "{exchange}");
        assert!(!exchange.contains("authorization: Basic"), "{exchange}");
    }

    /// And the profile's own parameters, which the identity provider
    /// evaluates its policy against: the grant is minted for the resource's
    /// authorization server, naming the MCP server it is meant to buy a token
    /// for.
    #[tokio::test]
    async fn it_pins_the_audience_and_resource_of_the_exchange() {
        let (addr, seen) = spawn_identity_provider(r#""client_secret_basic""#).await;

        let assertion = IdentityAssertion::new(format!("http://{addr}"), "idp-app", "id.token")
            .with_client_secret("s3cret")
            .require_https(false);

        assertion.assertion(exchange_request()).await.unwrap();

        let requests = seen.lock().unwrap().clone();
        let exchange = requests
            .iter()
            .find(|request| request.contains("POST /token"))
            .expect("the exchange must have reached the provider");

        assert!(
            exchange
                .contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Atoken-exchange"),
            "{exchange}"
        );
        assert!(exchange.contains("subject_token=id.token"), "{exchange}");
        assert!(
            exchange
                .contains("requested_token_type=urn%3Aietf%3Aparams%3Aoauth%3Atoken-type%3Aid-jag"),
            "{exchange}"
        );
        assert!(
            exchange.contains("audience=https%3A%2F%2Fauth.example.com"),
            "the grant is audienced to the resource's authorization server: {exchange}"
        );
        assert!(
            exchange.contains("resource=https%3A%2F%2Fmcp.example.com%2Fmcp"),
            "and names the MCP server it is for: {exchange}"
        );
        // Basic here, because that is what this provider advertised.
        assert!(exchange.contains("authorization: Basic"), "{exchange}");
    }

    #[test]
    fn it_keeps_credentials_out_of_debug_output() {
        let assertion = IdentityAssertion::new("https://idp.example", "idp-app", "id.token")
            .with_client_secret("s3cret");

        let rendered = format!("{assertion:?}");

        assert!(!rendered.contains("s3cret"));
        assert!(!rendered.contains("id.token"));
        assert!(rendered.contains("[redacted]"));
    }
}

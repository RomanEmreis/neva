//! How the client identifies itself, and to whom.
//!
//! The MCP spec gives a client three ways to have a `client_id`, in priority
//! order: one it was configured with, a Client ID Metadata Document the
//! authorization server dereferences, or Dynamic Client Registration. This is
//! where that choice is configured and where the combinations that cannot work
//! are refused -- a metadata document paired with a client secret, say, since a
//! document describes a public client.
//!
//! [`OAuthClientConfig::validate`] runs when the client is built, so a
//! configuration that could never complete a flow fails where it was written
//! rather than at the first `401`.

use super::*;

/// OAuth client configuration, set with
/// [`HttpClient::with_oauth`](crate::transport::http::HttpClient::with_oauth).
///
/// Everything is optional: without scopes the resource's advertised
/// `scopes_supported` are requested; tokens live in an in-process store
/// and the interactive step runs through [`LoopbackHandler`] unless
/// replaced.
///
/// # Obtaining a `client_id`
///
/// MCP defines three registration mechanisms and a priority order among
/// them, which this configuration follows:
///
/// 1. [`with_client_id`](Self::with_client_id) -- credentials issued out of
///    band by one authorization server (pre-registration). Used whenever
///    they are configured. Bind them to their server with
///    [`with_issuer`](Self::with_issuer).
/// 2. [`with_client_id_document`](Self::with_client_id_document) -- a Client
///    ID Metadata Document (CIMD): an https URL the authorization server
///    dereferences for the client's metadata. Used when the server
///    advertises `client_id_metadata_document_supported`.
/// 3. Dynamic Client Registration (RFC 7591), the fallback when neither is
///    configured or the server does not support CIMD. **Deprecated** by the
///    2026-07-28 spec and retained for servers that offer nothing else.
///
/// # Example
/// ```no_run
/// use neva::Client;
///
/// let mut client = Client::new()
///     .with_options(|opt| opt
///         .with_http(|http| http
///             .with_oauth(|oauth| oauth.with_scopes(["mcp:tools"]))
///         )
///     );
/// ```
pub struct OAuthClientConfig {
    pub(super) client_id: Option<String>,
    pub(super) client_secret: Option<String>,
    #[cfg(feature = "client-oauth-jwt")]
    pub(super) private_key_jwt: Option<PrivateKeyJwt>,
    pub(super) client_id_document: Option<String>,
    pub(super) jwks_uri: Option<String>,
    pub(super) issuer: Option<String>,
    pub(super) scopes: Option<Vec<String>>,
    pub(super) require_https: bool,
    pub(super) grant: ClientGrant,
    pub(super) store: Arc<dyn TokenStore>,
    pub(super) handler: Arc<dyn AuthorizationHandler>,
}

impl std::fmt::Debug for OAuthClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthClientConfig")
            .field("client_id", &self.client_id)
            .field("client_id_document", &self.client_id_document)
            .field("jwks_uri", &self.jwks_uri)
            .field("issuer", &self.issuer)
            .field("scopes", &self.scopes)
            .field("require_https", &self.require_https)
            .field("grant", &self.grant)
            .finish()
    }
}

impl Default for OAuthClientConfig {
    fn default() -> Self {
        Self {
            client_id: None,
            client_secret: None,
            #[cfg(feature = "client-oauth-jwt")]
            private_key_jwt: None,
            client_id_document: None,
            jwks_uri: None,
            issuer: None,
            scopes: None,
            require_https: true,
            grant: ClientGrant::AuthorizationCode,
            store: Arc::new(InMemoryTokenStore::new()),
            handler: Arc::new(LoopbackHandler::new()),
        }
    }
}

impl OAuthClientConfig {
    /// Uses a pre-registered OAuth client id instead of registering.
    ///
    /// Pre-registered credentials belong to one authorization server; name
    /// it with [`with_issuer`](Self::with_issuer) so a server that later
    /// points at a different one is refused rather than handed credentials
    /// it never issued.
    ///
    /// # Example
    /// ```no_run
    /// use neva::Client;
    ///
    /// let mut client = Client::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_oauth(|oauth| oauth
    ///                 .with_client_id("mcp-cli")
    ///                 .with_issuer("https://auth.example.com"))
    ///         )
    ///     );
    /// ```
    pub fn with_client_id(mut self, client_id: impl Into<String>) -> Self {
        self.client_id = Some(client_id.into());
        self
    }

    /// Identifies this client with a Client ID Metadata Document: `url` is
    /// both the `client_id` sent to the authorization server and the https
    /// location it dereferences for the client's metadata
    /// ([draft-ietf-oauth-client-id-metadata-document-00](https://datatracker.ietf.org/doc/html/draft-ietf-oauth-client-id-metadata-document-00)).
    ///
    /// This is the forward path for a client and server with no prior
    /// relationship, and needs no registration request: the server fetches
    /// the document instead. Used when the authorization server advertises
    /// `client_id_metadata_document_supported`, and otherwise only when the
    /// server has said nothing either way and offers no registration endpoint
    /// -- there being nothing else left to try. A server that answered `false`
    /// has stated it cannot resolve a URL id, so the flow registers
    /// dynamically instead of spending a browser round on an id that will be
    /// refused.
    ///
    /// `url` must use the `https` scheme and carry a path component. It is
    /// checked when the client connects, so a malformed one fails there
    /// rather than mid-flow. A Client ID Metadata Document describes a
    /// *public* client, so pairing this with
    /// [`with_client_secret`](Self::with_client_secret) is rejected.
    ///
    /// Hosting the document is the deployer's job -- it is a static file.
    /// Generate its contents with
    /// [`client_metadata_document`](Self::client_metadata_document) so what
    /// is published cannot drift from what the flow sends.
    ///
    /// # Example
    /// ```no_run
    /// use neva::Client;
    ///
    /// let mut client = Client::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_oauth(|oauth| oauth
    ///                 .with_client_id_document("https://app.example.com/mcp-client.json"))
    ///         )
    ///     );
    /// ```
    pub fn with_client_id_document(mut self, url: impl Into<String>) -> Self {
        self.client_id_document = Some(url.into());
        self
    }

    /// Names the authorization server the configured credentials belong to,
    /// by its `issuer` identifier.
    ///
    /// A `client_id` is issued by one authorization server and means nothing
    /// at another, and neither does a refresh token. Naming the issuer is
    /// what lets this client tell "the same server as before" from "the
    /// resource now points somewhere else": pre-registered credentials meeting
    /// a different issuer fail with an error instead of being presented to a
    /// server that never issued them, and a stored refresh token is only
    /// offered to the server that minted it.
    ///
    /// It is also what the [`TokenStore`] entry is filed under, so credentials
    /// from two different servers never share a slot and a stored refresh
    /// token is only ever read back under the server that minted it. Migrating
    /// therefore leaves the old server's tokens where they are rather than
    /// offering them to the new one, which is the whole point.
    ///
    /// Without it the credentials are unbound: they still work against a
    /// server that never changes its authorization server, but a stored
    /// refresh token is not reused across a restart, since nothing records
    /// which server it came from.
    ///
    /// # Example
    /// ```no_run
    /// use neva::Client;
    ///
    /// let mut client = Client::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_oauth(|oauth| oauth
    ///                 .with_client_id("mcp-cli")
    ///                 .with_issuer("https://auth.example.com"))
    ///         )
    ///     );
    /// ```
    pub fn with_issuer(mut self, issuer: impl Into<String>) -> Self {
        self.issuer = Some(issuer.into());
        self
    }

    /// Makes this a confidential client authenticating to the token
    /// endpoint with `client_secret`. Only meaningful together with
    /// [`with_client_id`](Self::with_client_id).
    ///
    /// Sent as HTTP Basic credentials (RFC 6749 section 2.3.1) unless the
    /// authorization server advertises only `client_secret_post`, in which
    /// case it travels in the request body instead. A server that accepts
    /// neither fails the flow rather than having the secret sent a way it
    /// said it would refuse.
    ///
    /// # Example
    /// ```no_run
    /// use neva::Client;
    ///
    /// let mut client = Client::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_oauth(|oauth| oauth
    ///                 .with_client_id("mcp-cli")
    ///                 .with_client_secret("s3cret"))
    ///         )
    ///     );
    /// ```
    pub fn with_client_secret(mut self, secret: impl Into<String>) -> Self {
        self.client_secret = Some(secret.into());
        self
    }

    /// Authenticates to the token endpoint with a `private_key_jwt` client
    /// assertion (RFC 7523 section 2.2) rather than a shared secret: the
    /// client signs a short-lived JWT with `key`, and nothing it holds ever
    /// leaves the process.
    ///
    /// This is what the client-credentials extension RECOMMENDS over a
    /// secret, and the [CIMD draft section 6.2](https://www.ietf.org/archive/id/draft-ietf-oauth-client-id-metadata-document-00.html#section-6.2)
    /// is what makes it usable without pre-registration -- the same document
    /// that carries the client's metadata carries the public key the server
    /// verifies with, which
    /// [`client_metadata_document`](Self::client_metadata_document) publishes
    /// for you when the key was given one with `with_public_jwk`.
    ///
    /// The assertion *is* the credential, so pairing this with
    /// [`with_client_secret`](Self::with_client_secret) is rejected rather
    /// than quietly resolved in the assertion's favour.
    ///
    /// # Example
    /// ```no_run
    /// use neva::Client;
    /// use neva::auth::oauth::{ClientError, JwsAlgorithm, PrivateKeyJwt};
    ///
    /// # fn run(pem: &[u8]) -> Result<(), ClientError> {
    /// let key = PrivateKeyJwt::from_pem(pem, JwsAlgorithm::ES256)?;
    ///
    /// let mut client = Client::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_oauth(|oauth| oauth
    ///                 .with_client_id("mcp-service")
    ///                 .with_private_key_jwt(key)
    ///                 .with_client_credentials())
    ///         )
    ///     );
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "client-oauth-jwt")]
    pub fn with_private_key_jwt(mut self, key: PrivateKeyJwt) -> Self {
        self.private_key_jwt = Some(key);
        self
    }

    /// Publishes this client's public keys at `url` instead of inside its
    /// Client ID Metadata Document, and names that location in the document
    /// [`client_metadata_document`](Self::client_metadata_document) builds.
    ///
    /// An authorization server that has never registered this client learns
    /// which key verifies its assertions from the document alone, so a
    /// document declaring `private_key_jwt` has to carry the material one way
    /// or the other: embedded, by giving the key its public half with
    /// `PrivateKeyJwt::with_public_jwk`, or referenced, by this. The
    /// [CIMD draft section 6.2](https://www.ietf.org/archive/id/draft-ietf-oauth-client-id-metadata-document-00.html#section-6.2)
    /// shows the referenced form, and it is what lets keys rotate at one
    /// hosted location instead of by republishing the document.
    ///
    /// Only read when building that document -- the flow itself never fetches
    /// it, since it holds the private half already.
    ///
    /// The two forms are exclusive: RFC 7591 section 2 has `jwks_uri` and
    /// `jwks` MUST NOT both appear, so a key already carrying its public half
    /// through `PrivateKeyJwt::with_public_jwk` publishes by value and needs
    /// none of this. Configuring both is refused rather than resolved.
    ///
    /// # Example
    /// ```no_run
    /// use neva::auth::oauth::OAuthClientConfig;
    ///
    /// let config = OAuthClientConfig::default()
    ///     .with_client_id_document("https://app.example.com/mcp-client.json")
    ///     .with_jwks_uri("https://app.example.com/jwks.json");
    /// ```
    pub fn with_jwks_uri(mut self, url: impl Into<String>) -> Self {
        self.jwks_uri = Some(url.into());
        self
    }

    /// Obtains tokens with the client credentials grant (RFC 6749
    /// section 4.4) instead of the authorization-code flow: the client
    /// authenticates as itself and no user is involved.
    ///
    /// The `io.modelcontextprotocol/oauth-client-credentials` extension.
    /// Everything before the token request is unchanged -- the `401`, the
    /// Protected Resource Metadata, the authorization server metadata -- and
    /// the browser round is simply not there, so the configured
    /// [`AuthorizationHandler`] is never called.
    ///
    /// Credentials for this flow are established out of band, so the client
    /// id has to be configured: dynamic registration is not used here, and a
    /// flow that reached the token endpoint with a freshly minted public
    /// client would have nothing to authenticate with. Pair this with
    /// [`with_client_secret`](Self::with_client_secret) or, better,
    /// [`with_private_key_jwt`](Self::with_private_key_jwt).
    ///
    /// No refresh token is issued (RFC 6749 section 4.4.3), so re-running the
    /// grant is how the session renews -- which it does on its own, without a
    /// `401` to prompt it.
    ///
    /// # Example
    /// ```no_run
    /// use neva::Client;
    ///
    /// let mut client = Client::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_oauth(|oauth| oauth
    ///                 .with_client_id("mcp-service")
    ///                 .with_client_secret("s3cret")
    ///                 .with_client_credentials())
    ///         )
    ///     );
    /// ```
    pub fn with_client_credentials(mut self) -> Self {
        self.grant = ClientGrant::ClientCredentials;
        self
    }

    /// Obtains tokens by presenting a JWT some other authority issued, as an
    /// RFC 7523 section 2.1 authorization grant.
    ///
    /// The workload-identity profile: a client running as a workload already
    /// holds a credential its platform minted -- a projected service account
    /// token, a SPIFFE SVID -- and federates it to the MCP server's
    /// authorization server rather than holding a second, MCP-specific
    /// secret. `provider` supplies that JWT, and is asked again for every
    /// token request so a rotating credential is read fresh; a `String` is
    /// itself an [`AssertionProvider`], for one that does not rotate.
    ///
    /// A refusal here is final. The assertion was either accepted or it was
    /// not, so this client neither resends it nor falls back to another
    /// grant: the error surfaces, and fixing the assertion -- or the trust
    /// the server was configured with -- is what resolves it.
    ///
    /// # Example
    /// ```no_run
    /// use neva::Client;
    ///
    /// # fn run(workload_jwt: String) {
    /// let mut client = Client::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_oauth(|oauth| oauth
    ///                 .with_client_id("customer-router-agent")
    ///                 .with_jwt_bearer(workload_jwt))
    ///         )
    ///     );
    /// # }
    /// ```
    pub fn with_jwt_bearer(mut self, provider: impl AssertionProvider) -> Self {
        self.grant = ClientGrant::JwtBearer(Arc::new(provider));
        self
    }

    /// Obtains tokens through the enterprise-managed authorization profile:
    /// the identity assertion the enterprise identity provider issued at
    /// single sign-on is exchanged there for a cross-domain grant, which is
    /// then presented to the MCP server's authorization server.
    ///
    /// Sugar over [`with_jwt_bearer`](Self::with_jwt_bearer) --
    /// [`IdentityAssertion`] is the [`AssertionProvider`] that runs the
    /// RFC 8693 exchange -- so the same rule about a final refusal applies.
    ///
    /// The credentials configured here belong to the *MCP server's*
    /// authorization server, where the grant is presented; the ones on
    /// [`IdentityAssertion`] belong to the identity provider, where it is
    /// obtained. They are two registrations at two servers and are not
    /// interchangeable.
    ///
    /// # Example
    /// ```no_run
    /// use neva::Client;
    /// use neva::auth::oauth::IdentityAssertion;
    ///
    /// # fn run(id_token: String) {
    /// let mut client = Client::new()
    ///     .with_options(|opt| opt
    ///         .with_http(|http| http
    ///             .with_oauth(|oauth| oauth
    ///                 .with_client_id("mcp-app")
    ///                 .with_client_secret("s3cret")
    ///                 .with_identity_assertion(IdentityAssertion::new(
    ///                     "https://acme.idp.example", "idp-app", id_token)))
    ///         )
    ///     );
    /// # }
    /// ```
    pub fn with_identity_assertion(self, assertion: IdentityAssertion) -> Self {
        self.with_jwt_bearer(assertion)
    }

    /// Sets the scopes to request. Defaults to the resource's advertised
    /// `scopes_supported`.
    pub fn with_scopes<I, S>(mut self, scopes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.scopes = Some(scopes.into_iter().map(Into::into).collect());
        self
    }

    /// Controls whether plain `http://` discovery/token endpoints are
    /// rejected. Enabled by default; disable only against a local
    /// development issuer.
    pub fn require_https(mut self, required: bool) -> Self {
        self.require_https = required;
        self
    }

    /// Replaces the in-process token store with a custom
    /// [`TokenStore`] (encrypted file, OS keychain, ...).
    pub fn with_token_store(mut self, store: impl TokenStore + 'static) -> Self {
        self.store = Arc::new(store);
        self
    }

    /// Replaces the interactive step with a custom
    /// [`AuthorizationHandler`].
    pub fn with_handler(mut self, handler: impl AuthorizationHandler) -> Self {
        self.handler = Arc::new(handler);
        self
    }

    /// Builds the metadata document to publish at the URL configured with
    /// [`with_client_id_document`](Self::with_client_id_document), listing
    /// `redirect_uris` as the locations authorization responses may be
    /// delivered to.
    ///
    /// The document is the same one dynamic registration would have sent,
    /// plus the `client_id` the spec requires it to carry -- so hosting this
    /// is publishing exactly what the flow claims about itself. Serialize it
    /// as JSON and serve it as a static file.
    ///
    /// Every redirect URI the [`AuthorizationHandler`] may produce has to be
    /// listed: an authorization server validates the one it is sent against
    /// this list. A [`LoopbackHandler`] on an ephemeral port therefore cannot
    /// be described by any document -- pin it with
    /// [`with_port`](LoopbackHandler::with_port) and list both the
    /// `127.0.0.1` and `localhost` spellings of that port. A client running a
    /// grant that has no redirect -- client credentials, JWT bearer -- passes
    /// an empty list instead, since there is no authorization response to
    /// deliver anywhere.
    ///
    /// The `grant_types` follow whichever grant is configured, and a
    /// [`with_private_key_jwt`](Self::with_private_key_jwt) key is published
    /// as the document's `token_endpoint_auth_method`. That is what lets a
    /// client with no pre-registration authenticate at all: the server
    /// dereferences one URL and learns both who the client is and which key
    /// to verify with.
    ///
    /// Which is why a key has to come with the material to verify it --
    /// embedded as `jwks` by giving the key its public half with
    /// `PrivateKeyJwt::with_public_jwk`, or referenced as a `jwks_uri` with
    /// [`with_jwks_uri`](Self::with_jwks_uri). Exactly one of the two:
    /// publishing neither is refused here rather than hosted and then
    /// answered `invalid_client` on every token request, and publishing both
    /// is what RFC 7591 section 2 forbids outright.
    ///
    /// # Example
    /// ```no_run
    /// use neva::auth::oauth::OAuthClientConfig;
    ///
    /// let config = OAuthClientConfig::default()
    ///     .with_client_id_document("https://app.example.com/mcp-client.json");
    ///
    /// let document = config.client_metadata_document([
    ///     "http://127.0.0.1:8919/callback",
    ///     "http://localhost:8919/callback",
    /// ])?;
    ///
    /// println!("{}", serde_json::to_string_pretty(&document)?);
    /// # Ok::<(), neva::error::Error>(())
    /// ```
    pub fn client_metadata_document<I, S>(&self, redirect_uris: I) -> Result<ClientMetadata, Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let Some(client_id) = &self.client_id_document else {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "no client id document is configured; set one with `with_client_id_document`",
            ));
        };

        validate_client_id_document_url(client_id, self.require_https)?;

        let uris = redirect_uris
            .into_iter()
            .map(|uri| uri.as_ref().to_owned())
            .collect::<Vec<_>>();

        // Required for the redirect-based grant and meaningless for the
        // others: RFC 7591 section 2 makes `redirect_uris` REQUIRED for
        // clients using `authorization_code`, and a client-credentials client
        // has no authorization response to receive.
        if uris.is_empty() && self.grant.is_interactive() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "a client id document must list at least one redirect URI",
            ));
        }

        // `client_id` is what separates a metadata document from a
        // registration request: the server fetches the document and checks
        // that the id inside matches the URL it fetched. `ClientMetadata` does
        // not model the field -- a registration request never carries one --
        // so it travels as an extension, which serde flattens to the top level
        // where the server reads it.
        let mut metadata = registration_metadata_for(&uris)
            .with_grant_types(self.grant.registration_grant_types().iter().copied());

        if !self.grant.is_interactive() {
            metadata.response_types.clear();
        }

        if let Some(url) = &self.jwks_uri {
            metadata = metadata.with_jwks_uri(url.clone());
        }

        // A key supersedes the `none` a public client publishes: the
        // assertion is the credential, and the document is where a server
        // with no prior relationship learns to expect one.
        //
        // Which is also why it has to learn *which key*. A document that
        // declares `private_key_jwt` and carries neither `jwks` nor a
        // `jwks_uri` leaves a server that never registered this client with
        // no material to verify the assertion against, so every token request
        // it ever makes is answered `invalid_client` -- deterministically,
        // and only after the document has been published and a flow has run.
        // Refusing to emit one is the whole value of generating it here.
        #[cfg(feature = "client-oauth-jwt")]
        if let Some(key) = &self.private_key_jwt {
            metadata = metadata
                .with_token_endpoint_auth_method(client_auth::PRIVATE_KEY_JWT)
                .with_token_endpoint_auth_signing_alg(key.algorithm().as_str());

            match (key.jwks(), &self.jwks_uri) {
                // RFC 7591 section 2: "The `jwks_uri` and `jwks` parameters
                // MUST NOT both be present in the same request or response."
                // They are two answers to one question -- where this client's
                // keys are -- and a document giving both is nonconforming,
                // which a strict server may simply refuse. Which of the two
                // was meant is not something to guess at: dropping either one
                // silently would publish a key location its author did not
                // choose.
                (Some(_), Some(url)) => {
                    return Err(Error::new(
                        ErrorCode::InvalidRequest,
                        format!(
                            "a client id document publishes its keys either by value or \
                             by reference, not both: the signing key carries its public \
                             half *and* `{url}` is configured. Drop the `with_jwks_uri`, \
                             or build the key without `PrivateKeyJwt::with_public_jwk`"
                        ),
                    ));
                }
                (Some(jwks), None) => {
                    let jwks = serde_json::to_value(jwks)
                        .map_err(|err| Error::new(ErrorCode::InternalError, err.to_string()))?;
                    metadata = metadata.with_jwks(jwks);
                }
                // Named separately, which is the form the CIMD draft shows.
                (None, Some(_)) => {}
                (None, None) => {
                    return Err(Error::new(
                        ErrorCode::InvalidRequest,
                        "a client id document that authenticates with `private_key_jwt` has \
                         to publish the key that verifies its assertions: attach the public \
                         half to the key with `PrivateKeyJwt::with_public_jwk`, or host a \
                         key set and name it with `with_jwks_uri`",
                    ));
                }
            }
        }

        metadata.additional_fields.insert(
            "client_id".to_owned(),
            serde_json::Value::String(client_id.clone()),
        );

        Ok(metadata)
    }

    /// Whether this client has anything to authenticate to a token endpoint
    /// with.
    ///
    /// A `client_id` alone is identification, not authentication -- it is
    /// public by construction, which is why the authorization-code flow leans
    /// on PKCE instead.
    fn authenticates(&self) -> bool {
        #[cfg(feature = "client-oauth-jwt")]
        if self.private_key_jwt.is_some() {
            return true;
        }
        self.client_secret.is_some()
    }

    /// Fails the configuration that cannot produce a working flow, at the
    /// point the client is built rather than at the first `401`.
    pub(super) fn validate(&self) -> Result<(), Error> {
        // The assertion *is* the credential (RFC 7523 section 2.2), and a
        // secret alongside it is never sent. Resolving that silently in the
        // key's favour would leave an operator who meant to use the secret
        // with a flow that works for a reason they did not choose -- and one
        // that breaks the day the key is removed.
        #[cfg(feature = "client-oauth-jwt")]
        if self.private_key_jwt.is_some() && self.client_secret.is_some() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "`with_private_key_jwt` and `with_client_secret` are alternatives; \
                 a client authenticates with a signed assertion or with a secret, \
                 not both",
            ));
        }

        // A key says how this client authenticates, and dynamic registration
        // is the one identity that cannot carry it: the registration would
        // have to publish the public half for the server to verify against,
        // *and* the server would have to answer that it accepted
        // `private_key_jwt` rather than the `none` that was asked for.
        // Neither is knowable until a registration has been spent, and a
        // registered client that quietly does not use its key is the one
        // outcome nobody wants. A key is anyway the credential of a client
        // whose identity outlives a single registration -- which is what a
        // pre-registered id, or the metadata document the CIMD draft section
        // 6.2 pairs with exactly this method, is for.
        #[cfg(feature = "client-oauth-jwt")]
        if self.private_key_jwt.is_some()
            && self.client_id.is_none()
            && self.client_id_document.is_none()
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "`with_private_key_jwt` needs an identity that outlives a \
                 registration: name the pre-registered id with `with_client_id`, \
                 or publish the key alongside the client's metadata with \
                 `with_client_id_document`",
            ));
        }

        // Credentials for a client-authenticating grant are established out
        // of band -- the client-credentials extension says dynamic
        // registration is not used here -- so an unnamed client has nothing
        // to present at the token endpoint. Said now rather than after a
        // `401`, two discovery requests and a registration that could not
        // have helped.
        if !self.grant.is_interactive()
            && self.client_id.is_none()
            && self.client_id_document.is_none()
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                format!(
                    "the `{}` grant authenticates the client itself and is not \
                     registered for dynamically; configure the credentials it was \
                     issued with `with_client_id`",
                    self.grant.grant_type()
                ),
            ));
        }

        // RFC 6749 section 4.4 restricts this grant to confidential clients,
        // and the extension spells out the two ways to be one: a signed
        // assertion (RECOMMENDED) or a client secret. A client holding
        // neither reaches the token endpoint with nothing but its `client_id`
        // and is refused there -- deterministically, on every run -- so it is
        // refused here instead, where the credential would have been written.
        //
        // The JWT-bearer grant is deliberately not held to this. Its
        // assertion *is* the grant rather than the client's credential, and
        // the workload-identity profile has the client authenticate with
        // nothing at all: the authorization server trusts the issuer of the
        // assertion, not a registration.
        if matches!(self.grant, ClientGrant::ClientCredentials) && !self.authenticates() {
            // A document cannot be paired with a secret -- the rule below
            // says why -- so naming one here would send its author around a
            // second refusal.
            let remedy = match (&self.client_id_document, cfg!(feature = "client-oauth-jwt")) {
                (Some(_), true) => "`with_private_key_jwt`",
                (Some(_), false) => "`with_private_key_jwt`, under the `client-oauth-jwt` feature",
                (None, true) => "`with_client_secret` or `with_private_key_jwt`",
                (None, false) => {
                    "`with_client_secret`, or `with_private_key_jwt` under the \
                     `client-oauth-jwt` feature"
                }
            };

            return Err(Error::new(
                ErrorCode::InvalidRequest,
                format!(
                    "the `{}` grant authenticates the client itself, so this client \
                     needs a credential to present: set {remedy}",
                    self.grant.grant_type()
                ),
            ));
        }

        match (&self.client_id, &self.client_id_document) {
            (Some(_), Some(_)) => Err(Error::new(
                ErrorCode::InvalidRequest,
                "`with_client_id` and `with_client_id_document` are alternatives; \
                 configure the pre-registered id or the document URL, not both",
            )),
            // A document is fetched by any authorization server that meets
            // the URL, so a shared secret cannot be what it authenticates
            // with: there is nobody to have shared it with. Attaching one
            // would contradict the document while quietly sending the secret
            // anyway, so it is a configuration error rather than something to
            // honor. A `private_key_jwt` key is the exception the CIMD draft
            // section 6.2 makes -- the document publishes the public half, so
            // the credential is asymmetric and travels with the identity.
            (None, Some(url)) if self.client_secret.is_some() => Err(Error::new(
                ErrorCode::InvalidRequest,
                format!(
                    "a client id document cannot be paired with a client secret, \
                     since `{url}` is resolved by any authorization server that \
                     meets it; authenticate with `with_private_key_jwt` instead"
                ),
            )),
            (None, Some(url)) => validate_client_id_document_url(url, self.require_https),
            _ => Ok(()),
        }
    }

    /// What this client is *expected* to be identified by, before any
    /// discovery has happened -- the store key the warm start reads.
    ///
    /// A guess, because which mechanism actually runs depends on what the
    /// server advertises, and there is nothing to ask before the first
    /// request. When it turns out wrong -- a configured document meeting a
    /// server that resolves none, so the flow registers instead -- the read
    /// simply finds an empty slot, which is the right answer: what is stored
    /// there belongs to an identity this session will not be presenting.
    /// The flow then files its own credentials under
    /// [`ClientIdSource::persistent_id`], which is what actually ran, and the
    /// session moves onto that slot.
    pub(super) fn client_identity(&self) -> &str {
        self.client_id
            .as_deref()
            .or(self.client_id_document.as_deref())
            .unwrap_or_default()
    }

    /// Which of the three registration mechanisms identifies this client to
    /// `server`, in the priority order the spec sets out.
    pub(super) fn client_id_source(
        &self,
        server: &AuthorizationServerMetadata,
    ) -> ClientIdSource<'_> {
        if let Some(client_id) = &self.client_id {
            return ClientIdSource::PreRegistered(client_id);
        }

        // A client-authenticating grant has no third mechanism to fall back
        // to -- dynamic registration is not part of these profiles -- so the
        // document is the id, whatever the server advertises about resolving
        // one. The reasoning that makes silence decisive for the interactive
        // flow is that a browser round would be wasted on an id the server
        // cannot resolve; here there is no browser round, and trying buys a
        // plain `invalid_client` from the server that says so, which beats an
        // error this client invents about a server it has not asked.
        if !self.grant.is_interactive()
            && let Some(url) = &self.client_id_document
        {
            return ClientIdSource::Document(url);
        }

        let advertised = client_id_metadata_document_supported(server);
        match &self.client_id_document {
            // Advertised: the mechanism the spec puts ahead of registration.
            Some(url) if advertised == Some(true) => ClientIdSource::Document(url),
            // Said nothing either way, and offers no registration endpoint to
            // fall back to. The document is the only thing left to try, and a
            // server that resolves URL ids without advertising it -- the draft
            // is younger than the servers -- would accept it.
            //
            // A server that said `false` is not this case, however little else
            // it offers. It has stated it cannot resolve a URL id, so sending
            // one buys an `invalid_client` at best, and only after walking the
            // user through a browser first.
            Some(url) if advertised.is_none() && server.registration_endpoint.is_none() => {
                ClientIdSource::Document(url)
            }
            // Registering dynamically is what the spec has a client fall back
            // to when the server does not resolve metadata documents -- an id
            // it does not know how to resolve would simply be an unknown
            // client.
            _ => ClientIdSource::Dynamic,
        }
    }

    pub(super) fn client_config(&self) -> ClientConfig {
        ClientConfig::new().require_https(self.require_https)
    }
}

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
    pub(super) client_id_document: Option<String>,
    pub(super) issuer: Option<String>,
    pub(super) scopes: Option<Vec<String>>,
    pub(super) require_https: bool,
    pub(super) store: Arc<dyn TokenStore>,
    pub(super) handler: Arc<dyn AuthorizationHandler>,
}

impl std::fmt::Debug for OAuthClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthClientConfig")
            .field("client_id", &self.client_id)
            .field("client_id_document", &self.client_id_document)
            .field("issuer", &self.issuer)
            .field("scopes", &self.scopes)
            .field("require_https", &self.require_https)
            .finish()
    }
}

impl Default for OAuthClientConfig {
    fn default() -> Self {
        Self {
            client_id: None,
            client_secret: None,
            client_id_document: None,
            issuer: None,
            scopes: None,
            require_https: true,
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
    pub fn with_client_secret(mut self, secret: impl Into<String>) -> Self {
        self.client_secret = Some(secret.into());
        self
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
    /// `127.0.0.1` and `localhost` spellings of that port.
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

        if uris.is_empty() {
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
        let mut metadata = registration_metadata_for(&uris);
        metadata.additional_fields.insert(
            "client_id".to_owned(),
            serde_json::Value::String(client_id.clone()),
        );

        Ok(metadata)
    }

    /// Fails the configuration that cannot produce a working flow, at the
    /// point the client is built rather than at the first `401`.
    pub(super) fn validate(&self) -> Result<(), Error> {
        match (&self.client_id, &self.client_id_document) {
            (Some(_), Some(_)) => Err(Error::new(
                ErrorCode::InvalidRequest,
                "`with_client_id` and `with_client_id_document` are alternatives; \
                 configure the pre-registered id or the document URL, not both",
            )),
            // A document describes a public client -- it is fetched by any
            // authorization server that meets the URL, and the metadata this
            // client publishes says `token_endpoint_auth_method: "none"`.
            // Attaching a secret would contradict the document while quietly
            // sending the secret anyway, so it is a configuration error rather
            // than something to honor.
            (None, Some(url)) if self.client_secret.is_some() => Err(Error::new(
                ErrorCode::InvalidRequest,
                format!(
                    "a client id document describes a public client, so `{url}` \
                     cannot be paired with a client secret"
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

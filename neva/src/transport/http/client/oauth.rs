//! Client-side OAuth 2.1 authorization for the Streamable HTTP transport.
//!
//! Implements the MCP authorization sequence on top of
//! [`volga-oauth-client`](https://docs.rs/volga-oauth-client) (framework
//! independent -- plain hyper): a `401` challenge is parsed for its
//! `resource_metadata` pointer (RFC 9728 section 5.1), the Protected Resource
//! Metadata and the authorization server metadata are discovered
//! (RFC 8414, OIDC fallback), the client obtains a `client_id` through one of
//! the three registration mechanisms (see [`OAuthClientConfig`]), and the
//! authorization-code + PKCE flow runs with the server's canonical URI as the
//! RFC 8707 resource indicator. The callback is checked for `state` and the
//! RFC 9207 `iss` parameter before the code is exchanged.
//!
//! The interactive step is pluggable through [`AuthorizationHandler`];
//! the default [`LoopbackHandler`] serves desktop/CLI clients by opening
//! the system browser and capturing the redirect on a loopback listener.

use crate::shared::BoxFuture;
use std::sync::{Arc, RwLock};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Mutex,
};

use crate::error::{Error, ErrorCode};

use volga_oauth_client::{
    AuthorizationServerMetadata, BearerChallenge, ClientConfig, ClientError, DiscoveryClient,
    OAuthClient, RegistrationClient, canonicalize_resource_uri, protected_resource_metadata_url,
};
pub use volga_oauth_client::{ClientMetadata, InMemoryTokenStore, TokenSet, TokenStore};

/// Default time the [`LoopbackHandler`] waits for the user to complete
/// authorization in the browser.
const DEFAULT_AUTH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Client name sent with dynamic client registration when none is
/// configured.
const DEFAULT_CLIENT_NAME: &str = "neva MCP client";

/// The query parameters delivered to the redirect URI by the
/// authorization server.
///
/// Produced by an [`AuthorizationHandler`]; parse a raw query string
/// with [`CallbackParams::from_query`].
#[derive(Debug, Clone)]
pub struct CallbackParams {
    /// The authorization code to exchange for tokens.
    pub code: String,
    /// The `state` echoed back by the server (CSRF check).
    pub state: String,
    /// The issuer identifier per RFC 9207, when the server sends one.
    pub iss: Option<String>,
}

impl CallbackParams {
    /// Parses authorization-response query parameters
    /// (e.g. `"code=abc&state=xyz&iss=https%3A%2F%2Fauth"`).
    ///
    /// Returns an error when the response carries an OAuth `error`
    /// (RFC 6749 section 4.1.2.1) or is missing `code`/`state`.
    ///
    /// # Example
    /// ```no_run
    /// use neva::auth::oauth::CallbackParams;
    ///
    /// let params = CallbackParams::from_query("code=abc&state=xyz")?;
    /// assert_eq!(params.code, "abc");
    /// # Ok::<(), neva::error::Error>(())
    /// ```
    pub fn from_query(query: &str) -> Result<Self, Error> {
        let mut code = None;
        let mut state = None;
        let mut iss = None;
        let mut error = None;
        let mut error_description = None;

        for (key, value) in form_urlencoded_parse(query) {
            match key.as_str() {
                "code" => code = Some(value),
                "state" => state = Some(value),
                "iss" => iss = Some(value),
                "error" => error = Some(value),
                "error_description" => error_description = Some(value),
                _ => {}
            }
        }

        if let Some(error) = error {
            let description = error_description.unwrap_or_default();
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                format!("authorization failed: {error}: {description}"),
            ));
        }

        match (code, state) {
            (Some(code), Some(state)) => Ok(Self { code, state, iss }),
            _ => Err(Error::new(
                ErrorCode::InvalidRequest,
                "authorization response is missing `code` or `state`",
            )),
        }
    }
}

/// Minimal `application/x-www-form-urlencoded` pair iterator -- enough
/// for authorization-response queries (no `+`-space legacy handling
/// beyond the standard).
fn form_urlencoded_parse(query: &str) -> impl Iterator<Item = (String, String)> + '_ {
    query.split('&').filter_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        Some((percent_decode(key)?, percent_decode(value)?))
    })
}

/// Percent-decodes a query component (with `+` as space).
fn percent_decode(s: &str) -> Option<String> {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes.get(i + 1..i + 3)?;
                let hex = std::str::from_utf8(hex).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// The interactive step of the authorization-code flow: how the
/// authorization URL is presented to the user and how the redirect
/// callback comes back.
///
/// The default [`LoopbackHandler`] covers desktop/CLI clients. A web or
/// headless embedder implements this trait to route the URL through its
/// own UI and deliver the callback parameters however they arrive.
///
/// Both methods return a [`BoxFuture`] rather than being `async fn`: the
/// handler is stored behind `Arc<dyn AuthorizationHandler>`, and `async fn`
/// in a trait is not dyn-compatible. `Box::pin(async move { ... })` is all an
/// implementation needs -- and the alias is neva's own, so implementing this
/// trait pulls in no `futures` dependency.
///
/// # Example
/// ```no_run
/// use neva::auth::oauth::{AuthorizationHandler, CallbackParams};
/// use neva::error::Error;
/// use neva::shared::BoxFuture;
///
/// struct MyUi;
///
/// impl AuthorizationHandler for MyUi {
///     fn redirect_uri(&self) -> BoxFuture<'_, Result<String, Error>> {
///         Box::pin(async { Ok("https://my.app/oauth/callback".into()) })
///     }
///     fn authorize(&self, url: String) -> BoxFuture<'_, Result<CallbackParams, Error>> {
///         Box::pin(async move {
///             // show `url` to the user, await the callback...
///             # let _ = url;
///             todo!()
///         })
///     }
/// }
/// ```
pub trait AuthorizationHandler: Send + Sync + 'static {
    /// The redirect URI the authorization response will be delivered to.
    ///
    /// Called once per flow, before dynamic client registration -- the
    /// URI is registered and sent with the authorization request.
    fn redirect_uri(&self) -> BoxFuture<'_, Result<String, Error>>;

    /// Presents `authorization_url` to the user and returns the callback
    /// parameters once the authorization server redirects back.
    fn authorize(&self, authorization_url: String) -> BoxFuture<'_, Result<CallbackParams, Error>>;
}

/// Default [`AuthorizationHandler`] for desktop/CLI clients: binds a
/// loopback listener, opens the system browser at the authorization URL
/// and captures the single redirect request.
///
/// The redirect URI is `http://127.0.0.1:{port}/callback` -- loopback
/// redirects are the standard exception to the HTTPS rule for native
/// clients, and dynamic registration declares such a client as
/// `application_type: "native"` accordingly.
///
/// # Example
/// ```no_run
/// use neva::Client;
/// use neva::auth::oauth::LoopbackHandler;
///
/// let mut client = Client::new()
///     .with_options(|opt| opt
///         .with_http(|http| http
///             .with_oauth(|oauth| oauth
///                 .with_handler(LoopbackHandler::new().with_port(8919)))
///         )
///     );
/// ```
pub struct LoopbackHandler {
    port: u16,
    open_browser: bool,
    timeout: std::time::Duration,
    listener: Mutex<Option<TcpListener>>,
}

impl std::fmt::Debug for LoopbackHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoopbackHandler")
            .field("port", &self.port)
            .field("open_browser", &self.open_browser)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl Default for LoopbackHandler {
    fn default() -> Self {
        Self {
            port: 0,
            open_browser: true,
            timeout: DEFAULT_AUTH_TIMEOUT,
            listener: Mutex::new(None),
        }
    }
}

impl LoopbackHandler {
    /// Creates a handler listening on an ephemeral loopback port.
    pub fn new() -> Self {
        Self::default()
    }

    /// Pins the callback listener to a fixed port -- required when the
    /// authorization server does not allow arbitrary loopback ports on
    /// the registered redirect URI.
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Disables launching the system browser; the authorization URL is
    /// only logged. For environments that surface the URL elsewhere.
    pub fn without_browser(mut self) -> Self {
        self.open_browser = false;
        self
    }

    /// Sets how long to wait for the user to complete authorization.
    ///
    /// Default: 5 minutes.
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = timeout;
        self
    }

    async fn accept_callback(&self) -> Result<CallbackParams, Error> {
        let listener = self.listener.lock().await.take().ok_or_else(|| {
            Error::new(
                ErrorCode::InternalError,
                "loopback listener is not bound; `redirect_uri` must be called first",
            )
        })?;

        let (mut stream, _) = listener.accept().await.map_err(Error::from)?;

        // The callback is a single short GET; the request line is all we
        // need, but read up to the header terminator to be a good citizen.
        let mut buf = vec![0u8; 8192];
        let mut len = 0;
        loop {
            let n = stream.read(&mut buf[len..]).await.map_err(Error::from)?;
            len += n;
            if n == 0 || len == buf.len() || buf[..len].windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }

        let params = parse_callback_request(&buf[..len]);
        let (status, body) = match &params {
            Ok(_) => (
                "200 OK",
                "<html><body><h3>Authorization complete.</h3>You can close this tab and return to the application.</body></html>",
            ),
            Err(_) => (
                "400 Bad Request",
                "<html><body><h3>Authorization failed.</h3>Check the application logs.</body></html>",
            ),
        };
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        // Best-effort: the response only makes the browser tab friendly.
        let _ = stream.write_all(resp.as_bytes()).await;
        let _ = stream.shutdown().await;

        params
    }
}

/// Extracts the query string out of the callback's request line
/// (`GET /callback?code=...&state=... HTTP/1.1`) and parses it.
fn parse_callback_request(raw: &[u8]) -> Result<CallbackParams, Error> {
    let line = raw
        .split(|&b| b == b'\r' || b == b'\n')
        .next()
        .unwrap_or_default();
    let line = std::str::from_utf8(line)
        .map_err(|_| Error::new(ErrorCode::InvalidRequest, "malformed callback request"))?;
    let target = line
        .split(' ')
        .nth(1)
        .ok_or_else(|| Error::new(ErrorCode::InvalidRequest, "malformed callback request"))?;
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or_default();
    CallbackParams::from_query(query)
}

impl AuthorizationHandler for LoopbackHandler {
    fn redirect_uri(&self) -> BoxFuture<'_, Result<String, Error>> {
        Box::pin(async move {
            let listener = TcpListener::bind(("127.0.0.1", self.port))
                .await
                .map_err(Error::from)?;
            let port = listener.local_addr().map_err(Error::from)?.port();
            *self.listener.lock().await = Some(listener);
            Ok(format!("http://127.0.0.1:{port}/callback"))
        })
    }

    fn authorize(&self, authorization_url: String) -> BoxFuture<'_, Result<CallbackParams, Error>> {
        Box::pin(async move {
            #[cfg(feature = "tracing")]
            tracing::info!(logger = "neva", "authorize at: {authorization_url}");

            if self.open_browser {
                open_in_browser(&authorization_url);
            }

            tokio::time::timeout(self.timeout, self.accept_callback())
                .await
                .map_err(|_| Error::new(ErrorCode::InternalError, "authorization timed out"))?
        })
    }
}

/// Launches the system browser at `url`, best-effort -- on failure the
/// URL is still available from the log/handler.
fn open_in_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let result: std::io::Result<std::process::Child> = Err(std::io::Error::other(
        "no known browser launcher for this platform",
    ));

    if let Err(_err) = result {
        #[cfg(feature = "tracing")]
        tracing::warn!(logger = "neva", "failed to open the browser: {_err}");
    }
}

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
    client_id: Option<String>,
    client_secret: Option<String>,
    client_id_document: Option<String>,
    issuer: Option<String>,
    scopes: Option<Vec<String>>,
    require_https: bool,
    store: Arc<dyn TokenStore>,
    handler: Arc<dyn AuthorizationHandler>,
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
    fn validate(&self) -> Result<(), Error> {
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

    /// Which of the three registration mechanisms identifies this client to
    /// `server`, in the priority order the spec sets out.
    fn client_id_source<'a>(&'a self, server: &AuthorizationServerMetadata) -> ClientIdSource<'a> {
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

    fn client_config(&self) -> ClientConfig {
        ClientConfig::new().require_https(self.require_https)
    }
}

/// The OAuth client and authorization-server metadata retained from the
/// last successful flow -- everything a non-interactive token refresh
/// needs.
struct FlowState {
    client: OAuthClient,
    metadata: AuthorizationServerMetadata,
}

/// How early before expiration a stored access token is proactively
/// refreshed. Mirrors the leeway `OAuthClient::token` applies, so the
/// cheap staleness probe and the actual refresh decision agree.
const REFRESH_LEEWAY: std::time::Duration = std::time::Duration::from_secs(30);

/// Per-connection OAuth state: the current access token and the
/// single-flight authorization flow.
pub(crate) struct OAuthSession {
    config: OAuthClientConfig,
    /// Canonicalized server URL -- the RFC 8707 resource indicator and the
    /// discovery base.
    resource: String,
    /// Where this session's credentials live in the [`TokenStore`]: the
    /// resource, prefixed by the issuer they came from when one is configured.
    ///
    /// It starts out naming the *configured* issuer, because it is read before
    /// any discovery has happened, and moves to the issuer a flow actually ran
    /// against -- which is the slot that flow filed its tokens in. The two
    /// differ only where a portable identity is allowed to outlive a migration
    /// ([`OAuthSession::store_key_for`]); left at the configured one, the
    /// staleness probe would keep looking into an empty slot and every renewal
    /// would have to wait for a `401` to notice.
    store_key: RwLock<Arc<str>>,
    /// Current bearer token, read on every outgoing request.
    token: RwLock<Option<Arc<str>>>,
    /// Serializes authorization flows (concurrent 401s run one flow) and
    /// caches the client + metadata for non-interactive refresh.
    flow: Mutex<Option<FlowState>>,
    /// Scopes the last completed flow asked for.
    ///
    /// A re-authorization asks for these *plus* whatever the new challenge
    /// demands (SEP-2350): a token minted for the challenged scope alone would
    /// lose access the session already had, and the next call for the old scope
    /// would challenge straight back.
    requested_scopes: RwLock<Vec<String>>,
}

impl std::fmt::Debug for OAuthSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthSession")
            .field("resource", &self.resource)
            .finish()
    }
}

impl OAuthSession {
    /// Builds a session for the MCP server at `server_url`.
    pub(crate) fn new(config: OAuthClientConfig, server_url: &str) -> Result<Self, Error> {
        // Before anything else, so a configuration that cannot produce a
        // working flow is reported where it was written rather than at the
        // first `401` -- which may be a long-running process away.
        config.validate()?;

        let resource = canonicalize_resource_uri(server_url)
            .map_err(|err| Error::new(ErrorCode::InternalError, err.to_string()))?;

        let store_key = Self::initial_store_key(&config, &resource);

        let token = config
            .store
            .get(&store_key)
            .filter(|tokens| !tokens.is_expired())
            .map(|tokens| tokens.access_token.into());

        Ok(Self {
            config,
            resource,
            store_key: RwLock::new(store_key.into()),
            token: RwLock::new(token),
            flow: Mutex::new(None),
            requested_scopes: RwLock::new(Vec::new()),
        })
    }

    /// The key this session reads before it has discovered anything.
    fn store_key(&self) -> Arc<str> {
        self.store_key
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Moves the session onto the slot `issuer` files its tokens in, so the
    /// pre-discovery reads that follow -- the staleness probe, the stored
    /// grant -- look where the last flow actually wrote.
    fn record_issuer(&self, issuer: &str) {
        let key = self.store_key_for(issuer);
        let mut current = self
            .store_key
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if **current != *key {
            *current = Arc::from(&*key);
        }
    }

    /// Where this session's credentials live in the [`TokenStore`].
    ///
    /// The issuer is part of the key whenever one is configured, which is the
    /// spec's own prescription -- credentials are to be associated with the
    /// authorization server that issued them, "keyed by the authorization
    /// server's `issuer` identifier". Keying by the resource alone records
    /// nothing about where a stored credential came from, so after a migration
    /// the *current* configuration is all a check has to go on -- and the
    /// current configuration is exactly what an operator updates when the
    /// resource moves. The old server's refresh token would then be read back
    /// under the same key and offered to the new one. Under this key it is not
    /// found at all: it lives under the issuer that minted it, and nothing
    /// looks there again.
    ///
    /// The client id is deliberately not part of it. A stale one is refused by
    /// the server that issued the token rather than leaking it to another, so
    /// it costs a round trip, not a credential -- and a dynamically registered
    /// client has no id to key by until the flow it is about to run mints one.
    ///
    /// Unbound, the key is the resource alone, exactly as before: such a
    /// session never refreshes from the store
    /// ([`Self::may_reuse_stored_refresh`]), so its entry is only ever the
    /// warm start's access token -- audience-bound to the resource and only
    /// ever presented there, which is why an unlabelled slot is safe.
    ///
    /// This one is built from the *configured* issuer, because it is read
    /// before any discovery has happened. Once a flow knows which server it is
    /// actually talking to, [`Self::store_key_for`] is what files what that
    /// server minted.
    fn initial_store_key(config: &OAuthClientConfig, resource: &str) -> String {
        match &config.issuer {
            Some(issuer) => format!("{issuer}|{resource}"),
            None => resource.to_owned(),
        }
    }

    /// Where credentials minted by `issuer` belong -- the key every read and
    /// write from inside a flow uses, once discovery has named the server.
    ///
    /// It is the discovered issuer rather than the configured one because the
    /// key is a statement about where these tokens *came from*, and the two
    /// part company exactly where a portable identity is allowed to: a Client
    /// ID Metadata Document resolves at whichever server meets it, so a CIMD
    /// client whose resource has moved completes its flow against a server the
    /// configuration does not name. Filing that server's tokens under the
    /// configured issuer would mislabel them -- and if the resource ever moved
    /// back, the configured key would hand the *old* server a refresh token
    /// the *new* one minted, which is the leak the keying exists to stop.
    ///
    /// An unbound session has one unlabelled slot and keeps it, so that its
    /// warm start still finds what its own flow wrote.
    fn store_key_for(&self, issuer: &str) -> std::borrow::Cow<'_, str> {
        match self.config.issuer {
            Some(_) => std::borrow::Cow::Owned(format!("{issuer}|{}", self.resource)),
            None => std::borrow::Cow::Borrowed(&self.resource),
        }
    }

    /// Scopes this session is known to hold, most authoritative source first.
    ///
    /// The in-memory set records what the last flow *in this process* was
    /// granted, so it is empty after a restart -- and a persistent
    /// [`TokenStore`] hands back a token whose grant the process never saw. Left
    /// at that, the first `insufficient_scope` challenge after a restart would
    /// build its step-up from the demanded scopes alone and trade away
    /// everything the restored token already carried, which is the opposite of
    /// what SEP-2350 asks for. So a stored token's own `scope` -- what RFC 6749
    /// has the authorization server report as *granted* -- stands in for it.
    ///
    /// A server may omit `scope` when it granted exactly what was asked
    /// (RFC 6749 section 5.1), leaving nothing recorded. Configured scopes
    /// answer that case: they are what every flow of this session requests, so
    /// they are held by construction.
    fn requested_scopes(&self) -> Vec<String> {
        let asked = self
            .requested_scopes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if !asked.is_empty() {
            return asked;
        }

        self.config
            .store
            .get(&self.store_key())
            .and_then(|tokens| tokens.scope)
            .map(|granted| split_scopes(&granted))
            .filter(|granted| !granted.is_empty())
            .or_else(|| self.config.scopes.clone())
            .unwrap_or_default()
    }

    fn set_requested_scopes(&self, scopes: Vec<String>) {
        *self
            .requested_scopes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = scopes;
    }

    /// The current bearer token, if any.
    pub(crate) fn bearer(&self) -> Option<Arc<str>> {
        self.token
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn set_token(&self, token: Arc<str>) {
        *self
            .token
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(token);
    }

    /// The bearer token to attach to the next request, proactively
    /// refreshed when the stored set is about to expire and a refresh
    /// token is available -- the session then renews without user
    /// interaction. Falls back to the current token when refresh is not
    /// possible; the `401` path handles the rest.
    pub(crate) async fn refreshed_bearer(&self) -> Option<Arc<str>> {
        // Cheap staleness probe before taking the flow lock.
        let stale = self
            .config
            .store
            .get(&self.store_key())
            .is_some_and(|tokens| tokens.expires_within(REFRESH_LEEWAY));

        if !stale {
            return self.bearer();
        }

        let mut flow = self.flow.lock().await;
        self.maintain(&mut flow).await.or_else(|| self.bearer())
    }

    /// Non-interactive token maintenance through the cached client:
    /// serves the stored set, refreshing it when stale (rotation
    /// carry-over and dead-entry pruning included, via
    /// `OAuthClient::token`). Returns `None` when interactive
    /// authorization is required or no flow has completed yet.
    async fn maintain(&self, state: &mut Option<FlowState>) -> Option<Arc<str>> {
        let FlowState { client, metadata } = state.as_ref()?;
        self.refresh_with(client, metadata).await
    }

    /// [`Self::maintain`] for a client and metadata held directly rather than
    /// cached -- what the reconstruct-after-restart path has in hand.
    async fn refresh_with(
        &self,
        client: &OAuthClient,
        metadata: &AuthorizationServerMetadata,
    ) -> Option<Arc<str>> {
        // What the grant was known to cover going in. A refresh response may
        // leave `scope` out when the grant is unchanged (RFC 6749 section 5.1),
        // and the renewed set *replaces* the stored one -- so a renewal would
        // otherwise erase the only record of what the token carries. The next
        // `insufficient_scope` challenge would then widen from nothing and
        // trade the grant away, which is the very thing SEP-2350 forbids. The
        // refresh token itself is carried over for the same reason one step
        // down, inside `OAuthClient::token`.
        let key = self.store_key_for(&metadata.issuer);
        let carried = self.config.store.get(&key).and_then(|tokens| tokens.scope);

        match client.token(&key, metadata).await {
            Ok(Some(mut tokens)) => {
                // What the renewed token covers: what the response said it
                // granted, or -- when it said nothing -- the grant it did not
                // restate.
                let granted = tokens.scope.clone().or(carried);
                if tokens.scope.is_none()
                    && let Some(scope) = granted.clone()
                {
                    tokens.scope = Some(scope);
                    self.config.store.put(&key, &tokens);
                }
                // And the in-memory record moves with it. A refresh may
                // *narrow* the grant, and this process's memory of the earlier,
                // wider one outranks the store -- so a challenge demanding
                // something the renewed token no longer carries would read as
                // already covered, take the single-flight shortcut, and hand
                // back that same token to be refused again on the request's one
                // retry.
                if let Some(scope) = granted.as_deref() {
                    let scopes = split_scopes(scope);
                    if !scopes.is_empty() {
                        self.set_requested_scopes(scopes);
                    }
                }
                // This slot is where the session's credentials live now, which
                // matters when the key moved: a portable identity may have
                // renewed against a server the configuration does not name.
                self.record_issuer(&metadata.issuer);
                let token: Arc<str> = tokens.access_token.into();
                self.set_token(token.clone());
                Some(token)
            }
            // Nothing renewable -- interactive authorization it is.
            Ok(None) => None,
            // Transient failure (issuer unreachable): keep the current
            // token and let the request outcome decide.
            Err(_err) => {
                #[cfg(feature = "tracing")]
                tracing::warn!(logger = "neva", "token refresh failed: {_err}");
                None
            }
        }
    }

    /// Runs the authorization flow triggered by a `401` and returns the
    /// fresh bearer token.
    ///
    /// `www_authenticate` is the challenge header value, when present --
    /// its `resource_metadata` pointer takes precedence over well-known
    /// derivation. `used` is the token the failed request carried:
    /// concurrent callers that lost the race simply pick up the token
    /// the winning flow produced.
    pub(crate) async fn authorize(
        &self,
        www_authenticate: Option<&str>,
        used: Option<&str>,
    ) -> Result<Arc<str>, Error> {
        let mut flight = self.flow.lock().await;

        let challenge = www_authenticate.and_then(|header| BearerChallenge::parse(header).ok());
        // Scopes the challenge demands that this session has never asked for.
        // A refresh cannot widen a grant, so their presence is what separates
        // "this token expired" from "this token is not enough" -- the second
        // needs the user back, however fresh the token is.
        let demanded = challenge
            .as_ref()
            .and_then(|challenge| challenge.scope())
            .map(split_scopes)
            .unwrap_or_default();

        // `insufficient_scope` is itself the statement that this grant is too
        // narrow, and RFC 6750 leaves the `scope` attribute optional -- so a
        // server may say it without naming what it wants. Reading only the
        // named scopes would call that "not a step-up", take the refresh path,
        // and spend the exchange's one retry on a token that is short by
        // exactly as much as before.
        let insufficient = challenge.as_ref().is_some_and(|challenge| {
            matches!(
                challenge.error(),
                Some(volga_oauth_client::OAuthErrorCode::InsufficientScope)
            )
        });

        // Read after the lock, so a flow that finished while this caller queued
        // behind it is already accounted for.
        let held = self.requested_scopes();
        let uncovered = demanded.iter().any(|scope| !held.contains(scope));

        let step_up = insufficient || uncovered;

        // A step-up that named no scope leaves nothing to check coverage
        // against, and a token that merely changed proves nothing: a refresh
        // rotates the access token without touching what it covers, and any
        // other request in this process may have run one while this caller
        // queued. Taking it would be the refresh path under another name --
        // exactly what reading `insufficient_scope` was meant to stop -- and the
        // exchange's one retry would go out just as short as before.
        let unverifiable = step_up && demanded.is_empty();

        // Someone else may have completed a widening flow while this caller
        // waited on the lock, and its token is right here. Taking it is the
        // whole point of the single-flight lock: two callers refused for the
        // same missing scope must not walk the user through consent twice.
        //
        // Trustworthy only because both halves are checked: the grant on record
        // now covers what the challenge demanded, *and* the token is not the one
        // that was just refused.
        if !uncovered
            && !unverifiable
            && let Some(current) = self.bearer()
            && used != Some(&*current)
        {
            return Ok(current);
        }

        // A configured set is the caller's decision about what this client may
        // ever ask for, and the flow below honors it to the letter -- so a
        // challenge naming something outside it describes a grant this client
        // cannot obtain. Running the flow anyway is the worst of both: it
        // interrupts the user for consent and still comes back without the one
        // scope the call needed, so the retry is refused exactly as before.
        // Widening past the configured set is no answer either -- it would
        // override the decision, and an authorization server refuses a scope
        // the client is not registered for. So this ends here, naming the
        // scope, because adding it to `with_scopes` is the only thing that
        // resolves it.
        if step_up && let Some(configured) = &self.config.scopes {
            let missing = demanded
                .iter()
                .filter(|scope| !configured.contains(scope))
                .cloned()
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    format!(
                        "the server requires scope `{}`, which this client is not \
                         configured to request; add it to `with_scopes`",
                        missing.join(" ")
                    ),
                ));
            }
        }

        // Refresh before interrupting the user: a stored refresh token
        // renews the session silently. A token identical to the rejected
        // one is no help though (revoked server-side) -- interactive then.
        if !step_up
            && let Some(token) = self.maintain(&mut flight).await
            && used != Some(&*token)
        {
            return Ok(token);
        }

        let stated = challenge
            .as_ref()
            .and_then(|challenge| challenge.resource_metadata().map(str::to_owned));

        let discovery = DiscoveryClient::with_config(self.config.client_config());
        let resource_metadata = match stated {
            // The challenge named the document: that is the answer, and a
            // failure there is the failure -- guessing elsewhere would be
            // discovering a document the server did not point at.
            Some(url) => discovery
                .fetch_resource_metadata_from_url(&url, Some(&self.resource))
                .await
                .map_err(flow_error)?,
            None => self.discover_resource_metadata(&discovery).await?,
        };

        let server_metadata = discovery
            .discover_authorization_server(&resource_metadata)
            .await
            .map_err(flow_error)?;

        let source = self.config.client_id_source(&server_metadata);
        self.check_issuer_binding(source, &server_metadata)?;

        // Nothing configured, nowhere to register, and the server does not
        // resolve document URLs: there is no way for this client to obtain an
        // id here, and the spec's last resort -- ask a human for one -- means
        // saying so. Said now, before a redirect listener is bound and long
        // before a browser opens on a flow that ends in `invalid_client`.
        if source == ClientIdSource::Dynamic && server_metadata.registration_endpoint.is_none() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                format!(
                    "`{}` supports neither dynamic client registration nor client id \
                     metadata documents, so this client cannot obtain a client id there; \
                     register one out of band and configure it with `with_client_id`",
                    server_metadata.issuer
                ),
            ));
        }

        let redirect_uri = self.config.handler.redirect_uri().await?;
        let client = self
            .build_client(source, &server_metadata, &redirect_uri)
            .await?;

        // A durable [`TokenStore`] outlives the process; the flow state that
        // knows how to use it does not. So after a restart the refresh attempt
        // above found nothing to refresh *with* -- no client, no metadata --
        // and a stored refresh token, still perfectly good, went unused while
        // the user was walked through consent again. Both halves have just been
        // rebuilt, so ask once more before that.
        if !step_up
            && self.may_reuse_stored_refresh(source, &server_metadata)
            && let Some(token) = self.refresh_with(&client, &server_metadata).await
            && used != Some(&*token)
        {
            // Keep what made it work, so the next refresh is the cheap path.
            *flight = Some(FlowState {
                client,
                metadata: server_metadata,
            });
            return Ok(token);
        }

        // What to ask for, most specific first. A configured set is the
        // caller's decision and overrides everything -- and by here it already
        // covers whatever the challenge demanded, since a demand outside it
        // ended this call above. Otherwise the challenge names what this very
        // request needed, which is narrower and more current than the
        // resource's advertised set; `scopes_supported` is the fallback, and an
        // empty one means asking for no `scope` at all.
        let mut scopes = match &self.config.scopes {
            Some(configured) => configured.clone(),
            None if !demanded.is_empty() => demanded.clone(),
            None => resource_metadata.scopes_supported.clone(),
        };
        // SEP-2350: carry everything earlier rounds asked for, so a step-up
        // widens the grant instead of trading one scope for another.
        for held in self.requested_scopes() {
            if !scopes.contains(&held) {
                scopes.push(held);
            }
        }

        // The RFC 8707 resource indicator is the identifier the *accepted*
        // metadata declares, not the endpoint this client happens to talk to.
        // They are the same thing whenever the document was found under the
        // endpoint's own path -- that is what validating it checks -- but a
        // document served at the origin describes the origin, and asking for a
        // token audienced to the endpoint would either be refused by an
        // authorization server that enforces its own advertised identifier, or
        // grant a token for an audience the resource never claimed.
        let request = client
            .authorization_request(&server_metadata)
            .with_scopes(scopes.clone())
            .with_resource(resource_metadata.resource.clone())
            .build()
            .map_err(flow_error)?;

        let params = self.config.handler.authorize(request.url.clone()).await?;

        if !request.matches_state(&params.state) {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "authorization response `state` mismatch",
            ));
        }
        validate_issuer(&params, &server_metadata)?;

        let tokens = client
            .exchange_code(&server_metadata, &params.code, &request)
            .await
            .map_err(flow_error)?;

        // What the server *granted*, which is not always what was asked for.
        // RFC 6749 section 5.1 has the token response state `scope` whenever it
        // differs from the request and omit it when it matches, so the response
        // is the authority and the request is only the fallback. Recording the
        // request would count a scope that was asked for and refused as held --
        // and then the challenge that names it reads as "this token expired"
        // rather than "this grant is too narrow", so the client refreshes into
        // the same refusal instead of widening.
        let mut tokens = tokens;
        let granted = tokens
            .scope
            .as_deref()
            .map(split_scopes)
            .filter(|granted| !granted.is_empty())
            .unwrap_or(scopes);

        // A grant inferred from the request goes into the stored set too, not
        // just into memory. The store is what outlives the process, and the
        // omission that produced this inference -- "granted exactly what you
        // asked for" -- is the common case, so leaving it unwritten would have
        // the next run start out believing it holds nothing and let the first
        // step-up replace the grant instead of widening it.
        if tokens.scope.is_none() && !granted.is_empty() {
            tokens.scope = Some(granted.join(" "));
        }

        self.config
            .store
            .put(&self.store_key_for(&server_metadata.issuer), &tokens);

        self.record_issuer(&server_metadata.issuer);

        // Keep the client + metadata so future refreshes stay
        // non-interactive.
        *flight = Some(FlowState {
            client,
            metadata: server_metadata,
        });
        self.set_requested_scopes(granted);

        let token: Arc<str> = tokens.access_token.into();
        self.set_token(token.clone());
        Ok(token)
    }

    /// Refuses credentials that belong to a different authorization server
    /// than the one the resource now points at.
    ///
    /// A `client_id` obtained out of band is issued *by* one authorization
    /// server; it identifies nothing at another. So when the resource's
    /// metadata starts naming a different issuer, presenting it there is at
    /// best an `invalid_client` refusal and at worst hands an attacker-run
    /// server a credential and the user's consent for it. The spec has the
    /// client surface an error instead, and only the client knows which
    /// server its configured credentials came from -- hence
    /// [`with_issuer`](OAuthClientConfig::with_issuer).
    ///
    /// A Client ID Metadata Document URL is deliberately exempt: it is
    /// resolved by whichever server meets it, so it is portable across them
    /// by design and a change of issuer asks nothing of it. Dynamic
    /// registration is exempt for the opposite reason -- the id is minted
    /// against this very server, moments from now.
    fn check_issuer_binding(
        &self,
        source: ClientIdSource<'_>,
        metadata: &AuthorizationServerMetadata,
    ) -> Result<(), Error> {
        let ClientIdSource::PreRegistered(client_id) = source else {
            return Ok(());
        };
        let Some(bound_to) = &self.config.issuer else {
            return Ok(());
        };
        if bound_to == &metadata.issuer {
            return Ok(());
        }

        Err(Error::new(
            ErrorCode::InvalidRequest,
            format!(
                "client `{client_id}` is registered with `{bound_to}`, but \
                 `{}` now names `{}` as its authorization server; \
                 credentials are not portable between them",
                self.resource, metadata.issuer
            ),
        ))
    }

    /// Whether the refresh token sitting in the store may be offered to
    /// `metadata`'s token endpoint.
    ///
    /// Two things have to hold, and neither is implied by the other. The
    /// client id must be the one the token was issued to, which rules out
    /// dynamic registration: the id this flow is about to mint is not the one
    /// from last time, and a refresh token belongs to the client it was
    /// issued to. And the token has to have come from *this* authorization
    /// server -- a refresh token is a bearer credential for the endpoint that
    /// minted it, so sending it to a server that did not is handing that
    /// server a credential for another one.
    ///
    /// The second is settled by [`Self::store_key_for`] rather than here: the
    /// read goes to the slot `metadata`'s issuer files its own tokens in, so
    /// whatever comes back came from the server it is about to be sent to. All
    /// that is left to check is whether this session's slots carry an issuer
    /// at all. Unbound they do not -- there is one unlabelled slot, and a
    /// credential out of it proves nothing about where it came from, so the
    /// session re-authorizes interactively instead: a worse experience and the
    /// only safe answer.
    ///
    /// Deliberately *not* a comparison against the configured issuer. That
    /// would refuse a portable identity whose resource has migrated -- a CIMD
    /// client is allowed to complete a flow against a server its configuration
    /// does not name, and refusing to renew what that flow obtained would walk
    /// the user through consent on every restart. The slot it renews from is
    /// labelled with that same server, which is the assurance the comparison
    /// was standing in for.
    fn may_reuse_stored_refresh(
        &self,
        source: ClientIdSource<'_>,
        metadata: &AuthorizationServerMetadata,
    ) -> bool {
        if !source.survives_a_restart() {
            return false;
        }

        if self.config.issuer.is_none() {
            #[cfg(feature = "tracing")]
            tracing::debug!(
                logger = "neva",
                "not offering the stored refresh token to {}: the credentials name no \
                 issuer, so nothing says it came from there. Set `with_issuer` to reuse it.",
                metadata.issuer
            );
            return false;
        }

        true
    }

    /// Finds the Protected Resource Metadata for a server that issued a `401`
    /// without saying where it lives.
    ///
    /// RFC 9728 puts the document under the resource's own path
    /// (`/.well-known/oauth-protected-resource/mcp` for a server at `/mcp`), so
    /// that is asked first. A server that hosts one MCP endpoint often serves it
    /// at the root instead, which is a location the path-based derivation never
    /// reaches -- so a `404` falls back there rather than failing the flow over
    /// a document that exists.
    ///
    /// Strictly a `404`, and not "the first attempt did not work out". Any
    /// other failure means the path-based location answered, and what it said
    /// stands: falling back past a malformed body or a mismatched `resource`
    /// would trade an authoritative refusal for a document describing something
    /// else.
    async fn discover_resource_metadata(
        &self,
        discovery: &DiscoveryClient,
    ) -> Result<volga_oauth_client::ProtectedResourceMetadata, Error> {
        let path_based = protected_resource_metadata_url(&self.resource)
            .map_err(|err| Error::new(ErrorCode::InternalError, err.to_string()))?;

        let first = discovery
            .fetch_resource_metadata_from_url(&path_based, Some(&self.resource))
            .await;

        let Err(err) = first else {
            return first.map_err(flow_error);
        };

        // Only "there is no document here" opens the fallback. Every other
        // failure is the path-based document *answering*, and its answer is the
        // authoritative one: a body that does not parse, a `resource` that
        // names something else, a rejected plain-HTTP URL, a TLS or connection
        // failure. Treating those as absence would let a document that failed
        // validation be replaced by one from the origin, which is how a client
        // ends up authorizing against metadata for a different resource than
        // the one it just refused.
        if !matches!(err, ClientError::Http(status) if status.as_u16() == 404) {
            return Err(flow_error(err));
        }

        let Some(origin) = origin_of(&self.resource) else {
            return Err(flow_error(err));
        };

        let root = format!("{origin}{WELL_KNOWN_PROTECTED_RESOURCE}");
        if root == path_based {
            return Err(flow_error(err));
        }

        #[cfg(feature = "tracing")]
        tracing::debug!(
            logger = "neva",
            "no resource metadata at {path_based}; trying {root}"
        );

        // Checked against the origin, not against the endpoint. A document at
        // the root describes the whole origin as the protected resource -- that
        // is what puts it there rather than under the endpoint's path -- so
        // demanding it name the endpoint would reject every document this
        // fallback exists to find. The binding it does keep is the one that
        // matters: the document has to name the origin it was served from.
        discovery
            .fetch_resource_metadata_from_url(&root, Some(&origin))
            .await
            // Both attempts are named: reporting only one of them leaves the
            // reader guessing which location was the problem.
            .map_err(|root_err| {
                Error::new(
                    ErrorCode::InternalError,
                    format!(
                        "OAuth flow failed: no usable resource metadata \
                         at {path_based} ({err}) or {root} ({root_err})"
                    ),
                )
            })
    }

    /// Builds the [`OAuthClient`] for the identity `source` names: a
    /// configured id, a Client ID Metadata Document URL, or one obtained
    /// through dynamic registration (RFC 7591).
    async fn build_client(
        &self,
        source: ClientIdSource<'_>,
        server_metadata: &AuthorizationServerMetadata,
        redirect_uri: &str,
    ) -> Result<OAuthClient, Error> {
        let client = match source {
            ClientIdSource::PreRegistered(client_id) => {
                let mut client = OAuthClient::new(client_id);
                if let Some(secret) = &self.config.client_secret {
                    client = client.with_secret(secret.clone());
                }
                client
            }
            // Nothing to register: the URL *is* the id, and the server
            // resolves it to the document the deployer published. A CIMD
            // client is public by construction, so no secret is attached even
            // if one were configured -- and `OAuthClientConfig::validate`
            // refuses that pairing before it can get this far.
            ClientIdSource::Document(url) => OAuthClient::new(url),
            ClientIdSource::Dynamic => {
                let registration = RegistrationClient::with_config(self.config.client_config());
                let response = registration
                    .register(server_metadata, &registration_metadata(redirect_uri))
                    .await
                    .map_err(flow_error)?;
                OAuthClient::from_registration(&response).map_err(flow_error)?
            }
        };

        Ok(client
            .with_config(self.config.client_config())
            .with_redirect_uri(redirect_uri)
            .with_token_store(self.config.store.clone()))
    }
}

/// Builds the RFC 7591 registration document for a public
/// authorization-code client.
fn registration_metadata(redirect_uri: &str) -> ClientMetadata {
    registration_metadata_for(std::slice::from_ref(&redirect_uri))
}

/// [`registration_metadata`] over a set of redirect URIs -- what a hosted
/// Client ID Metadata Document needs, since it is written once and has to
/// cover every URI the handler may redirect to.
///
/// A loopback redirect URI makes this a **native** client
/// (`application_type: "native"`) -- authorization servers reject `web`
/// clients with plain-http loopback redirects, which is exactly the
/// desktop/CLI case.
fn registration_metadata_for<S: AsRef<str>>(redirect_uris: &[S]) -> ClientMetadata {
    let mut metadata = ClientMetadata::default()
        .with_redirect_uris(redirect_uris.iter().map(AsRef::as_ref))
        .with_grant_types(["authorization_code", "refresh_token"])
        .with_response_types(["code"])
        .with_token_endpoint_auth_method("none")
        .with_client_name(DEFAULT_CLIENT_NAME);

    // One loopback URI is enough: a client that redirects to loopback at all
    // is a native one, and declaring `web` would have the server reject that
    // URI. A document listing both spellings of the same loopback port is the
    // ordinary case and stays native.
    if redirect_uris
        .iter()
        .any(|uri| is_loopback_redirect(uri.as_ref()))
    {
        metadata = metadata.with_application_type("native");
    }

    metadata
}

/// What `server` says about resolving URL-formatted client ids into hosted
/// Client ID Metadata Documents -- `None` when it says nothing.
///
/// Silence and a stated `false` are different answers, and the difference
/// decides what a client with a document may try: see
/// [`OAuthClientConfig::client_id_source`].
///
/// Read out of the unmodelled fields because RFC 8414 does not define the
/// member; the CIMD draft adds it, and `volga-oauth-core` keeps anything it
/// does not model in `additional_fields`.
fn client_id_metadata_document_supported(server: &AuthorizationServerMetadata) -> Option<bool> {
    server
        .additional_fields
        .get("client_id_metadata_document_supported")
        .and_then(serde_json::Value::as_bool)
}

/// Checks a Client ID Metadata Document URL against the two requirements the
/// spec puts on it -- the `https` scheme and a path component -- and against
/// being a URL at all.
///
/// Both spec requirements are load-bearing. The scheme is what makes the
/// document's contents -- the redirect URIs an authorization server will
/// accept -- something an attacker on the path cannot rewrite. The path
/// component keeps a client id from naming a bare origin, which would make
/// every client hosted there the same client. `require_https(false)` relaxes
/// the first for a local development server, the same way it does for the
/// issuer's own endpoints.
///
/// The rest is what an authorization server has to be able to dereference,
/// which is the whole point of the value: full URI syntax through the
/// canonicalizer (scheme, IPv6 literals, percent-encoding, no userinfo, no
/// fragment) and a port in range. A client id this refuses is one no
/// conforming server could fetch, so refusing it here -- where it was
/// written -- beats a browser round ending in `invalid_client`.
fn validate_client_id_document_url(url: &str, require_https: bool) -> Result<(), Error> {
    let invalid = |reason: &str| {
        Err(Error::new(
            ErrorCode::InvalidRequest,
            format!("client id document URL `{url}` {reason}"),
        ))
    };

    // A fragment never reaches the server, so an id carrying one could never
    // match the document it is fetched from -- the match the server checks.
    // Named separately from the syntax check below, which would only call it
    // an invalid URI.
    if url.contains('#') {
        return invalid("must not carry a fragment");
    }

    // URI syntax, from the parser the rest of this module already trusts:
    // scheme, brackets around an IPv6 host, a numeric port, the characters a
    // path and query may hold. Checking those by hand is what lets
    // `https://[::1/client.json` or `https://example.com:bad/client.json`
    // through -- both have a non-empty something before the first `/` and
    // neither is a URL an authorization server could dereference.
    //
    // The *canonical* form is what gets inspected below, so the checks see
    // one spelling. It is deliberately not what gets sent: a Client ID
    // Metadata Document has to declare a `client_id` matching the URL the
    // server fetched, byte for byte, so lowercasing a host or dropping a
    // default port here would break the very match it exists for.
    let canonical = match canonicalize_resource_uri(url) {
        Ok(canonical) => canonical,
        Err(err) => return invalid(&format!("is not a valid URL: {err}")),
    };

    let rest = match canonical.split_once("://") {
        Some(("https", rest)) => rest,
        Some(("http", rest)) if !require_https => rest,
        Some(("http", _)) => {
            return invalid("must use the `https` scheme (or set `require_https(false)`)");
        }
        _ => return invalid("must be an absolute `https` URL"),
    };

    // The query goes first, because it may hold slashes of its own:
    // `https://example.com?location=/client.json` has no path component at all,
    // and looking for a slash across the whole of `rest` would find the one
    // inside the query and call it one.
    //
    // What is left is the authority and the path. Canonicalization drops a lone
    // root slash, so `https://example.com` and `https://example.com/` arrive
    // here alike -- both being the origin, which as a client id would make
    // every client hosted there the same client.
    let authority_and_path = rest.split('?').next().unwrap_or_default();
    let Some((authority, path)) = authority_and_path.split_once('/') else {
        return invalid("must contain a path component, e.g. `https://example.com/client.json`");
    };
    if path.is_empty() {
        return invalid("must contain a path component, e.g. `https://example.com/client.json`");
    }

    // The one thing the canonicalizer above does not settle. It holds the port
    // to digits, which keeps the authority well-formed, but not to a range --
    // and `:99999` is a number no socket has: every standard URL parser refuses
    // it, so the server that would fetch this document never gets as far as
    // trying.
    if let Some(port) = host_and_port(authority).1
        && port.parse::<u16>().is_err()
    {
        return invalid("must name a port in the 0-65535 range");
    }

    Ok(())
}

/// Which of MCP's three registration mechanisms supplies the `client_id` for
/// one authorization server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientIdSource<'a> {
    /// Issued out of band by one authorization server, and meaningless at
    /// any other.
    PreRegistered(&'a str),
    /// An https URL the server dereferences for a hosted metadata document.
    /// Portable: it is resolved on demand, so it needs no registration
    /// anywhere and stays valid when the resource changes servers.
    Document(&'a str),
    /// Registered per flow through RFC 7591 and never persisted, so nothing
    /// survives to be presented to the wrong server.
    Dynamic,
}

impl ClientIdSource<'_> {
    /// Whether this identity is the same on the next run of the process.
    ///
    /// A dynamically registered id is not: the next run registers again and
    /// gets another one, which is why credentials tied to the old id -- a
    /// stored refresh token above all -- are worthless after a restart.
    fn survives_a_restart(&self) -> bool {
        !matches!(self, Self::Dynamic)
    }
}

/// Whether `uri` redirects to a loopback interface (`127.0.0.1`,
/// `localhost` or `[::1]`), per the native-client loopback exception.
fn is_loopback_redirect(uri: &str) -> bool {
    let Some(rest) = uri
        .strip_prefix("http://")
        .or_else(|| uri.strip_prefix("https://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?']).next().unwrap_or_default();
    matches!(
        host_and_port(authority).0,
        "127.0.0.1" | "localhost" | "[::1]"
    )
}

/// Splits an authority into its host and its port, if it names one.
///
/// A bracketed IPv6 literal carries colons of its own, so the closing bracket
/// is what the host ends at; everything else ends at the last colon. An empty
/// port (`example.com:`) is no port at all.
fn host_and_port(authority: &str) -> (&str, Option<&str>) {
    match authority.split_once(']') {
        Some((bracketed, after)) => (
            &authority[..bracketed.len() + 1],
            after.strip_prefix(':').filter(|port| !port.is_empty()),
        ),
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host, (!port.is_empty()).then_some(port)),
            None => (authority, None),
        },
    }
}

/// Validates the RFC 9207 `iss` authorization-response parameter.
///
/// When the server metadata advertises
/// `authorization_response_iss_parameter_supported`, the parameter is
/// required and must match the issuer; when it is merely present, it
/// must still match. A mismatch means the response may come from a
/// different (potentially malicious) authorization server -- mix-up
/// attack -- and aborts the flow.
fn validate_issuer(
    params: &CallbackParams,
    metadata: &AuthorizationServerMetadata,
) -> Result<(), Error> {
    // A modelled field, so it never appears in `additional_fields` -- reading it
    // there made `supported` permanently false, and a server that advertised the
    // parameter and then omitted it from the redirect went unchallenged, which
    // is exactly the mix-up the parameter exists to catch.
    let supported = metadata.authorization_response_iss_parameter_supported;

    match (&params.iss, supported) {
        (Some(iss), _) if *iss != metadata.issuer => Err(Error::new(
            ErrorCode::InvalidRequest,
            format!(
                "authorization response `iss` mismatch: expected {}, got {iss}",
                metadata.issuer
            ),
        )),
        (None, true) => Err(Error::new(
            ErrorCode::InvalidRequest,
            "authorization server advertises RFC 9207 but the response carries no `iss`",
        )),
        _ => Ok(()),
    }
}

/// Maps a `volga-oauth-client` failure onto neva's error type.
fn flow_error(err: ClientError) -> Error {
    Error::new(
        ErrorCode::InternalError,
        format!("OAuth flow failed: {err}"),
    )
}

/// Splits an OAuth `scope` value into its space-delimited scope tokens.
fn split_scopes(scope: &str) -> Vec<String> {
    scope
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>()
}

/// RFC 9728's well-known path for Protected Resource Metadata.
const WELL_KNOWN_PROTECTED_RESOURCE: &str = "/.well-known/oauth-protected-resource";

/// The `scheme://authority` a resource identifier belongs to.
///
/// Returns `None` when `resource` is not a URL with an authority -- there is no
/// origin to hang a well-known path off then.
fn origin_of(resource: &str) -> Option<String> {
    let (scheme, rest) = resource.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;

    (!authority.is_empty()).then(|| format!("{scheme}://{authority}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_parses_callback_query() {
        let params =
            CallbackParams::from_query("code=abc&state=xyz&iss=https%3A%2F%2Fauth.example.com")
                .unwrap();
        assert_eq!(params.code, "abc");
        assert_eq!(params.state, "xyz");
        assert_eq!(params.iss.as_deref(), Some("https://auth.example.com"));
    }

    #[test]
    fn it_rejects_error_responses() {
        let err = CallbackParams::from_query("error=access_denied&error_description=nope&state=s")
            .unwrap_err();
        assert!(err.to_string().contains("access_denied"));
    }

    #[test]
    fn it_rejects_missing_code_or_state() {
        assert!(CallbackParams::from_query("code=abc").is_err());
        assert!(CallbackParams::from_query("state=xyz").is_err());
    }

    /// The token-endpoint futures must stay `Send`: neva drives them from
    /// spawned request tasks. volga-oauth-client 0.9.5 held a non-`Sync`
    /// `form_urlencoded::Serializer` across the await, which forced a
    /// `spawn_blocking` bridge here; 0.9.6 scopes it. Asserting the bound
    /// directly means a regression fails here rather than at some distant
    /// `tokio::spawn` call site.
    #[test]
    fn token_endpoint_futures_are_send() {
        fn assert_send<T: Send>(_: T) {}

        let client = OAuthClient::new("client-id");
        let metadata = as_metadata(None)
            .with_authorization_endpoint("https://auth.example.com/authorize")
            .with_token_endpoint("https://auth.example.com/token");
        let request = client
            .authorization_request(&metadata)
            .with_scopes(["openid"])
            .build()
            .unwrap();

        assert_send(client.exchange_code(&metadata, "code", &request));
        assert_send(client.refresh(&metadata, "refresh-token"));
    }

    #[test]
    fn an_authority_splits_into_a_host_and_a_port() {
        assert_eq!(host_and_port("example.com"), ("example.com", None));
        assert_eq!(
            host_and_port("example.com:8443"),
            ("example.com", Some("8443"))
        );
        // A bracketed IPv6 literal carries colons that are not the separator.
        assert_eq!(host_and_port("[::1]"), ("[::1]", None));
        assert_eq!(host_and_port("[::1]:9000"), ("[::1]", Some("9000")));
        // An empty port is no port -- and must not read as one, or the range
        // check would refuse a URL that names none.
        assert_eq!(host_and_port("example.com:"), ("example.com", None));
    }

    #[test]
    fn loopback_redirects_are_detected() {
        assert!(is_loopback_redirect("http://127.0.0.1:8919/callback"));
        assert!(is_loopback_redirect("http://localhost/callback"));
        assert!(is_loopback_redirect("http://[::1]:9000/callback"));
        assert!(!is_loopback_redirect("https://my.app/oauth/callback"));
        assert!(!is_loopback_redirect("res://localhost"));
    }

    #[test]
    fn loopback_registration_declares_a_native_client() {
        let metadata = registration_metadata("http://127.0.0.1:8919/callback");
        assert_eq!(metadata.application_type.as_deref(), Some("native"));
        assert_eq!(metadata.token_endpoint_auth_method.as_deref(), Some("none"));
        // The wire shape is what the AS actually reads -- it must stay a
        // top-level member, not an extension field.
        let json = serde_json::to_value(&metadata).unwrap();
        assert_eq!(json["application_type"], serde_json::json!("native"));
    }

    #[test]
    fn the_root_metadata_location_is_derived_from_the_origin() {
        assert_eq!(
            origin_of("https://api.example.com/mcp").as_deref(),
            Some("https://api.example.com")
        );
        assert_eq!(
            origin_of("http://127.0.0.1:8001/deep/path?x=1").as_deref(),
            Some("http://127.0.0.1:8001")
        );
        // Nothing to hang a well-known path off.
        assert!(origin_of("not-a-url").is_none());
        assert!(origin_of("https://").is_none());
    }

    #[test]
    fn scopes_split_on_whitespace() {
        assert_eq!(
            split_scopes("mcp:basic  mcp:write\tmcp:read"),
            ["mcp:basic", "mcp:write", "mcp:read"]
        );
        assert!(split_scopes("   ").is_empty());
    }

    #[test]
    fn web_registration_stays_a_web_client() {
        let metadata = registration_metadata("https://my.app/oauth/callback");
        assert!(metadata.application_type.is_none());
        let json = serde_json::to_value(&metadata).unwrap();
        assert!(json.get("application_type").is_none());
    }

    const CIMD_URL: &str = "https://app.example.com/mcp-client.json";

    /// The two requirements the spec puts on a CIMD `client_id`: the `https`
    /// scheme and a path component.
    #[test]
    fn a_client_id_document_url_must_be_https_with_a_path() {
        assert!(validate_client_id_document_url(CIMD_URL, true).is_ok());
        assert!(validate_client_id_document_url("https://example.com/c", true).is_ok());
        assert!(validate_client_id_document_url("https://example.com:8443/c", true).is_ok());
        assert!(validate_client_id_document_url("https://[::1]:8443/c.json", false).is_ok());

        assert!(validate_client_id_document_url("https://example.com/c?v=2", true).is_ok());

        // No path: the bare origin would make every client hosted there one
        // and the same client.
        assert!(validate_client_id_document_url("https://example.com", true).is_err());
        assert!(validate_client_id_document_url("https://example.com/", true).is_err());
        assert!(validate_client_id_document_url("https://example.com/?x=1", true).is_err());
        // Still the bare origin: the only slash belongs to the query.
        assert!(
            validate_client_id_document_url("https://example.com?to=/client.json", true).is_err()
        );

        // Digits alone do not make a port. `:99999` is a number no socket has,
        // and the URL parser on the server's side refuses it before it can
        // fetch anything.
        let err = validate_client_id_document_url("https://example.com:99999/c.json", true)
            .expect_err("a port outside the range must be refused");
        assert!(err.to_string().contains("0-65535"), "{err}");
        assert!(validate_client_id_document_url("https://example.com:65535/c.json", true).is_ok());
        // A fragment never reaches the server, so it could never match the
        // document it is fetched from.
        assert!(validate_client_id_document_url("https://example.com/c#f", true).is_err());
        assert!(validate_client_id_document_url("https://example.com/#f", true).is_err());
        assert!(validate_client_id_document_url("https:///client.json", true).is_err());
        assert!(validate_client_id_document_url("not-a-url", true).is_err());
        assert!(validate_client_id_document_url("client.json", true).is_err());

        // Plain http only under the same knob that admits a plain-http issuer.
        assert!(validate_client_id_document_url("http://localhost:9/c.json", true).is_err());
        assert!(validate_client_id_document_url("http://localhost:9/c.json", false).is_ok());
    }

    /// A malformed authority has a non-empty something before the first `/`,
    /// so a hand-rolled split calls it valid and the client is built around an
    /// id no authorization server can dereference. Only real URI parsing
    /// catches these.
    #[test]
    fn a_malformed_client_id_document_url_is_refused() {
        for url in [
            "https://[::1/client.json",           // unclosed IPv6 bracket
            "https://example.com:bad/c.json",     // port that is not a number
            "https://user@example.com/c.json",    // userinfo
            "https://exa mple.com/c.json",        // whitespace in the authority
            "https://exam\u{00a0}ple.com/c.json", // non-ASCII
        ] {
            let err = validate_client_id_document_url(url, true)
                .expect_err("a URL a server cannot fetch must be refused");
            assert!(err.to_string().contains("not a valid URL"), "{url}: {err}");
        }
    }

    /// The message has to name the fix, not just the refusal -- the URL is a
    /// deployment detail its author can correct in one edit.
    #[test]
    fn a_bad_client_id_document_url_fails_when_the_client_is_built() {
        let config = OAuthClientConfig::default().with_client_id_document("https://example.com");
        let err = OAuthSession::new(config, "https://api.example.com/mcp").unwrap_err();
        assert!(err.to_string().contains("path component"), "{err}");
    }

    /// A document says `token_endpoint_auth_method: "none"`; a secret says the
    /// opposite. Honoring the pair would send the secret while publishing that
    /// there is none.
    #[test]
    fn a_client_id_document_cannot_be_paired_with_a_secret() {
        let config = OAuthClientConfig::default()
            .with_client_id_document(CIMD_URL)
            .with_client_secret("s3cret");
        let err = OAuthSession::new(config, "https://api.example.com/mcp").unwrap_err();
        assert!(err.to_string().contains("public client"), "{err}");
    }

    #[test]
    fn a_pre_registered_id_and_a_document_are_alternatives() {
        let config = OAuthClientConfig::default()
            .with_client_id("mcp-cli")
            .with_client_id_document(CIMD_URL);
        let err = OAuthSession::new(config, "https://api.example.com/mcp").unwrap_err();
        assert!(err.to_string().contains("alternatives"), "{err}");
    }

    /// Authorization server metadata as it arrives on the wire, so the CIMD
    /// flag is read the way a real document delivers it -- through serde's
    /// flatten catch-all, since RFC 8414 does not model the member.
    fn as_supporting_cimd(supported: bool) -> AuthorizationServerMetadata {
        serde_json::from_value(serde_json::json!({
            "issuer": "https://auth.example.com",
            "response_types_supported": ["code"],
            "registration_endpoint": "https://auth.example.com/register",
            "client_id_metadata_document_supported": supported,
        }))
        .unwrap()
    }

    /// Silence and a stated `false` are different answers, and the difference
    /// is what a client with a document may try -- so the tri-state has to
    /// survive the wire.
    #[test]
    fn the_cimd_capability_is_read_off_the_wire_document() {
        assert_eq!(
            client_id_metadata_document_supported(&as_supporting_cimd(true)),
            Some(true)
        );
        assert_eq!(
            client_id_metadata_document_supported(&as_supporting_cimd(false)),
            Some(false)
        );
        assert_eq!(
            client_id_metadata_document_supported(&as_metadata(None)),
            None,
            "a server that never mentions the member has said nothing, not no"
        );
    }

    /// The spec's priority order: pre-registration first, then a metadata
    /// document when the server resolves them, then registration.
    #[test]
    fn the_client_id_source_follows_the_spec_priority_order() {
        let pre_registered = OAuthClientConfig::default().with_client_id("mcp-cli");
        assert_eq!(
            pre_registered.client_id_source(&as_supporting_cimd(true)),
            ClientIdSource::PreRegistered("mcp-cli"),
            "a configured id outranks everything the server advertises"
        );

        let document = OAuthClientConfig::default().with_client_id_document(CIMD_URL);
        assert_eq!(
            document.client_id_source(&as_supporting_cimd(true)),
            ClientIdSource::Document(CIMD_URL)
        );
        assert_eq!(
            document.client_id_source(&as_supporting_cimd(false)),
            ClientIdSource::Dynamic,
            "a server that does not resolve URL ids would see an unknown client"
        );

        assert_eq!(
            OAuthClientConfig::default().client_id_source(&as_supporting_cimd(true)),
            ClientIdSource::Dynamic,
            "with no document configured there is no URL to send"
        );
    }

    /// Falling back to registration is only an answer when registration is on
    /// offer. A server that has said *nothing* about metadata documents and
    /// offers nowhere to register leaves the document as the one thing left to
    /// try -- and it may well resolve one, the draft being younger than the
    /// servers.
    #[test]
    fn a_document_is_used_when_registration_is_not_on_offer() {
        let document = OAuthClientConfig::default().with_client_id_document(CIMD_URL);
        assert_eq!(
            document.client_id_source(&as_metadata(None)),
            ClientIdSource::Document(CIMD_URL)
        );
    }

    /// A server that answered `false`, though, has stated it cannot resolve a
    /// URL id. Sending one anyway buys an `invalid_client` -- after walking the
    /// user through a browser to get it.
    #[test]
    fn a_document_is_not_used_where_the_server_said_it_resolves_none() {
        let refuses = serde_json::from_value::<AuthorizationServerMetadata>(serde_json::json!({
            "issuer": "https://auth.example.com",
            "response_types_supported": ["code"],
            "client_id_metadata_document_supported": false,
        }))
        .unwrap();

        let document = OAuthClientConfig::default().with_client_id_document(CIMD_URL);
        assert_eq!(
            document.client_id_source(&refuses),
            ClientIdSource::Dynamic,
            "however little else the server offers"
        );
    }

    /// And with registration equally absent there is no mechanism left, which
    /// is worth saying plainly: the flow ends before a listener is bound,
    /// naming the one thing that resolves it.
    #[tokio::test]
    async fn a_server_offering_no_registration_mechanism_says_so() {
        let addr = spawn_bare_authorization_server().await;
        let resource = format!("http://{addr}/mcp");

        let config = OAuthClientConfig::default()
            .require_https(false)
            .with_client_id_document(CIMD_URL)
            .with_handler(NoInteraction);
        let session = OAuthSession::new(config, &resource).unwrap();

        let err = session
            .authorize(None, None)
            .await
            .expect_err("no mechanism can produce a client id here");
        assert!(err.to_string().contains("with_client_id"), "{err}");
    }

    /// What the deployer publishes has to be what the flow claims: the same
    /// builder, plus the `client_id` that makes it a metadata document.
    #[test]
    fn the_metadata_document_carries_the_client_id_and_its_redirect_uris() {
        let config = OAuthClientConfig::default().with_client_id_document(CIMD_URL);
        let document = config
            .client_metadata_document([
                "http://127.0.0.1:8919/callback",
                "http://localhost:8919/callback",
            ])
            .unwrap();

        let json = serde_json::to_value(&document).unwrap();
        assert_eq!(json["client_id"], serde_json::json!(CIMD_URL));
        assert_eq!(json["client_name"], serde_json::json!(DEFAULT_CLIENT_NAME));
        assert_eq!(
            json["redirect_uris"],
            serde_json::json!([
                "http://127.0.0.1:8919/callback",
                "http://localhost:8919/callback"
            ])
        );
        // Loopback redirects make it a native client here for the same reason
        // they do in a registration request.
        assert_eq!(json["application_type"], serde_json::json!("native"));
        assert_eq!(
            json["token_endpoint_auth_method"],
            serde_json::json!("none")
        );
    }

    #[test]
    fn a_metadata_document_needs_a_url_and_a_redirect_uri() {
        let no_url = OAuthClientConfig::default();
        assert!(
            no_url
                .client_metadata_document(["https://my.app/cb"])
                .is_err()
        );

        let no_redirect = OAuthClientConfig::default().with_client_id_document(CIMD_URL);
        assert!(
            no_redirect
                .client_metadata_document(Vec::<String>::new())
                .is_err()
        );
    }

    fn session(config: OAuthClientConfig) -> OAuthSession {
        OAuthSession::new(config, "https://api.example.com/mcp").unwrap()
    }

    /// A pre-registered `client_id` means nothing at a server that did not
    /// issue it, so a resource that starts naming another one ends the flow
    /// rather than presenting the credential there.
    #[test]
    fn pre_registered_credentials_are_refused_at_another_issuer() {
        let session = session(
            OAuthClientConfig::default()
                .with_client_id("mcp-cli")
                .with_issuer("https://auth.example.com"),
        );

        let same = as_metadata(None);
        let source = ClientIdSource::PreRegistered("mcp-cli");
        assert!(session.check_issuer_binding(source, &same).is_ok());

        let moved = AuthorizationServerMetadata::new("https://other.example.com");
        let err = session.check_issuer_binding(source, &moved).unwrap_err();
        assert!(err.to_string().contains("not portable"), "{err}");
        assert!(err.to_string().contains("other.example.com"), "{err}");
    }

    /// A metadata document is resolved by whichever server meets it, so a
    /// change of authorization server asks nothing of it. Registration mints
    /// its id against the server in front of it. Neither can be stale.
    #[test]
    fn portable_client_ids_survive_a_change_of_issuer() {
        let session = session(
            OAuthClientConfig::default()
                .with_client_id_document(CIMD_URL)
                .with_issuer("https://auth.example.com"),
        );
        let moved = AuthorizationServerMetadata::new("https://other.example.com");

        assert!(
            session
                .check_issuer_binding(ClientIdSource::Document(CIMD_URL), &moved)
                .is_ok()
        );
        assert!(
            session
                .check_issuer_binding(ClientIdSource::Dynamic, &moved)
                .is_ok()
        );
    }

    /// Without `with_issuer` nothing records which server issued the stored
    /// credentials, so an unbound one is left alone rather than offered to
    /// whichever server the resource names now.
    #[test]
    fn an_unbound_refresh_token_is_never_offered_to_anyone() {
        let source = ClientIdSource::PreRegistered("mcp-cli");

        let bound = session(
            OAuthClientConfig::default()
                .with_client_id("mcp-cli")
                .with_issuer("https://auth.example.com"),
        );
        assert!(bound.may_reuse_stored_refresh(source, &as_metadata(None)));

        let unbound = session(OAuthClientConfig::default().with_client_id("mcp-cli"));
        assert!(!unbound.may_reuse_stored_refresh(source, &as_metadata(None)));
    }

    /// What keeps a token from reaching the wrong server is the slot it lives
    /// in, not a comparison against the configuration. So a portable identity
    /// meeting a server its configuration does not name may still renew --
    /// from that server's own slot, which is a different one.
    #[test]
    fn a_migrated_portable_identity_renews_from_the_new_issuers_slot() {
        let session = session(
            OAuthClientConfig::default()
                .with_client_id_document(CIMD_URL)
                .with_issuer("https://auth.example.com"),
        );
        let moved = AuthorizationServerMetadata::new("https://other.example.com");

        assert!(session.may_reuse_stored_refresh(ClientIdSource::Document(CIMD_URL), &moved));
        assert_ne!(
            &*session.store_key_for(&moved.issuer),
            &*session.store_key(),
            "and not from the slot the stale configuration names"
        );
        assert!(
            session
                .store_key_for(&moved.issuer)
                .starts_with("https://other.example.com|"),
            "the slot is the one that server files its own tokens in"
        );
    }

    /// A dynamically registered id is not the one the stored token was issued
    /// to -- this flow is about to mint a different one -- so the token cannot
    /// be renewed under it however well the issuer matches.
    #[test]
    fn a_dynamically_registered_client_never_reuses_a_stored_refresh_token() {
        let session = session(OAuthClientConfig::default().with_issuer("https://auth.example.com"));
        assert!(!session.may_reuse_stored_refresh(ClientIdSource::Dynamic, &as_metadata(None)));
    }

    /// The flag is a *modelled* field, so it must be set through the builder:
    /// stashing it in `additional_fields` is what let these tests pass while the
    /// real document -- where serde puts it on the field -- read as unsupported.
    fn as_metadata(supported: Option<bool>) -> AuthorizationServerMetadata {
        let mut metadata = AuthorizationServerMetadata::new("https://auth.example.com");
        if let Some(supported) = supported {
            metadata = metadata.with_authorization_response_iss_parameter(supported);
        }
        metadata
    }

    /// The document a server actually sends, parsed the way the client parses
    /// it: the flag has to survive the round trip onto the modelled field.
    #[test]
    fn an_advertised_iss_parameter_survives_deserialization() {
        let doc = serde_json::json!({
            "issuer": "https://auth.example.com",
            "response_types_supported": ["code"],
            "authorization_response_iss_parameter_supported": true,
        });
        let metadata: AuthorizationServerMetadata = serde_json::from_value(doc).unwrap();
        assert!(metadata.authorization_response_iss_parameter_supported);
        assert!(
            validate_issuer(&callback(None), &metadata).is_err(),
            "a server that advertised `iss` and then omitted it must be refused"
        );
    }

    fn callback(iss: Option<&str>) -> CallbackParams {
        CallbackParams {
            code: "c".into(),
            state: "s".into(),
            iss: iss.map(str::to_owned),
        }
    }

    #[test]
    fn iss_mismatch_is_rejected() {
        let err = validate_issuer(
            &callback(Some("https://evil.example.com")),
            &as_metadata(None),
        )
        .unwrap_err();
        assert!(err.to_string().contains("mismatch"));
    }

    #[test]
    fn missing_iss_with_rfc9207_support_is_rejected() {
        assert!(validate_issuer(&callback(None), &as_metadata(Some(true))).is_err());
    }

    #[test]
    fn matching_iss_passes() {
        assert!(
            validate_issuer(
                &callback(Some("https://auth.example.com")),
                &as_metadata(Some(true))
            )
            .is_ok()
        );
    }

    #[test]
    fn missing_iss_without_support_passes() {
        assert!(validate_issuer(&callback(None), &as_metadata(None)).is_ok());
        assert!(validate_issuer(&callback(None), &as_metadata(Some(false))).is_ok());
    }

    #[tokio::test]
    async fn loopback_handler_round_trip() {
        let handler = LoopbackHandler::new().without_browser();
        let redirect = handler.redirect_uri().await.unwrap();
        assert!(redirect.starts_with("http://127.0.0.1:"));

        let addr = redirect
            .strip_prefix("http://")
            .and_then(|rest| rest.split('/').next())
            .unwrap()
            .to_owned();

        // Simulate the browser being redirected back by the AS.
        let callback = tokio::spawn(async move {
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            stream
                .write_all(b"GET /callback?code=abc&state=xyz HTTP/1.1\r\nHost: x\r\n\r\n")
                .await
                .unwrap();
            let mut resp = String::new();
            stream.read_to_string(&mut resp).await.unwrap();
            resp
        });

        let params = handler
            .authorize("http://unused.example".into())
            .await
            .unwrap();
        assert_eq!(params.code, "abc");
        assert_eq!(params.state, "xyz");

        let browser_view = callback.await.unwrap();
        assert!(browser_view.starts_with("HTTP/1.1 200"));
    }

    /// Serves one canned token-endpoint response over raw HTTP and
    /// returns the bound address.
    async fn spawn_token_endpoint(body: &'static str) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(resp.as_bytes()).await.unwrap();
        });
        addr
    }

    /// A one-shot HTTP server answering every request with `status` and `body`.
    async fn spawn_static(status: &'static str, body: &'static str) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        });
        addr
    }

    /// A server with one MCP endpoint that keeps its metadata at the root:
    /// `404` under the endpoint's path, and a document describing the origin at
    /// `/.well-known/oauth-protected-resource`.
    async fn spawn_root_document() -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let read = stream.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..read]).to_string();
                let root = format!("http://{addr}");
                let resp = if request.contains("/.well-known/oauth-protected-resource/mcp") {
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_string()
                } else {
                    let body =
                        format!(r#"{{"resource":"{root}","authorization_servers":["{root}"]}}"#);
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                };
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        });
        addr
    }

    /// The origin fallback exists for a server that keeps its one document at
    /// the root, which the path-based derivation never reaches. It must not
    /// exist for a path-based document that *answered* and was refused: falling
    /// back past a mismatched `resource` would authorize against metadata for a
    /// different resource than the one just rejected.
    #[tokio::test]
    async fn only_a_missing_document_opens_the_origin_fallback() {
        // Every path answers with a document naming a *different* resource, so
        // the path-based attempt fails validation rather than 404ing. The root
        // would "succeed" if the fallback were reached, since it is checked
        // against the origin -- which is exactly the confusion to avoid.
        let addr = spawn_static(
            "200 OK",
            r#"{"resource":"http://127.0.0.1:1","authorization_servers":["http://127.0.0.1:1"]}"#,
        )
        .await;

        let config = OAuthClientConfig::default().require_https(false);
        let session = OAuthSession::new(config, &format!("http://{addr}/mcp")).unwrap();
        let discovery = DiscoveryClient::with_config(session.config.client_config());

        let err = session
            .discover_resource_metadata(&discovery)
            .await
            .expect_err("a document that names another resource is not usable");
        // The path-based document's own verdict, verbatim -- not the combined
        // "at X or Y" message, which only the fallback path can produce.
        let msg = err.to_string();
        assert!(
            msg.contains("resource mismatch"),
            "the refusal must be the one the path-based document earned: {msg}"
        );
        assert!(
            !msg.contains("no usable resource metadata"),
            "the origin must not have been tried at all: {msg}"
        );

        // A genuine miss falls through to the origin, and what comes back is
        // the origin's own document -- `resource` included. That value is what
        // rides the authorization request as the RFC 8707 indicator, so an
        // authorization server enforcing its metadata's identifier sees the
        // resource that actually claimed the grant.
        let root_only = spawn_root_document().await;
        let config = OAuthClientConfig::default().require_https(false);
        let session = OAuthSession::new(config, &format!("http://{root_only}/mcp")).unwrap();
        let discovery = DiscoveryClient::with_config(session.config.client_config());

        let found = session
            .discover_resource_metadata(&discovery)
            .await
            .expect("the origin document answers");
        assert_eq!(
            found.resource,
            format!("http://{root_only}"),
            "the accepted document describes the origin, and says so"
        );

        // A genuine miss still falls through to the origin, and says so by
        // naming both locations when that fails too.
        let missing = spawn_static("404 Not Found", "{}").await;
        let config = OAuthClientConfig::default().require_https(false);
        let session = OAuthSession::new(config, &format!("http://{missing}/mcp")).unwrap();
        let discovery = DiscoveryClient::with_config(session.config.client_config());

        let err = session
            .discover_resource_metadata(&discovery)
            .await
            .expect_err("nothing is served at either location");
        let msg = err.to_string();
        assert!(
            msg.contains("/.well-known/oauth-protected-resource/mcp")
                && msg.contains("/.well-known/oauth-protected-resource ("),
            "a 404 must try the origin and report both: {msg}"
        );
    }

    /// RFC 9728 section 3.3 states two validation rules, and which applies
    /// depends on how the document was found. One reached by inserting the
    /// well-known suffix is checked against the identifier the suffix was
    /// inserted into, so a document at the origin legitimately names the
    /// origin. One reached through the challenge's `resource_metadata` pointer
    /// is checked against something else entirely: "the resource value returned
    /// MUST be identical to the URL that the client used to make the request to
    /// the resource server", and if they differ the document "MUST NOT be
    /// used". Section 7.3 says why -- it is what stops a server from pointing at
    /// a document that claims to speak for a resource it is not.
    ///
    /// So the same origin-wide document is usable when discovered and unusable
    /// when pointed at. That asymmetry is the rule, not an oversight, and this
    /// pins it: relaxing the pointed-at case to accept the origin would trade an
    /// impersonation check for the convenience of a server that is misusing the
    /// pointer.
    #[tokio::test]
    async fn a_challenge_pointer_is_held_to_the_url_the_client_called() {
        // The very document `only_a_missing_document_opens_the_origin_fallback`
        // accepts through discovery: it names the origin, and the endpoint this
        // client calls sits at `/mcp` under it.
        let addr = spawn_root_document().await;
        let config = OAuthClientConfig::default()
            .require_https(false)
            .with_handler(NoInteraction);
        let session = OAuthSession::new(config, &format!("http://{addr}/mcp")).unwrap();
        let challenge = format!(
            r#"Bearer resource_metadata="http://{addr}/.well-known/oauth-protected-resource""#
        );

        let err = session
            .authorize(Some(&challenge), None)
            .await
            .expect_err("a pointed-at document naming something other than the called URL");
        let msg = err.to_string();
        assert!(
            msg.contains("resource mismatch"),
            "the refusal must be the validation one, reached before any flow: {msg}"
        );
    }

    fn stale_tokens() -> TokenSet {
        TokenSet {
            access_token: "stale-token".into(),
            token_type: "Bearer".into(),
            refresh_token: Some("refresh-1".into()),
            scope: None,
            id_token: None,
            expires_at: Some(std::time::SystemTime::now()),
        }
    }

    fn session_with(store: Arc<dyn TokenStore>, flow: Option<FlowState>) -> OAuthSession {
        let config = OAuthClientConfig {
            store,
            ..OAuthClientConfig::default()
        };
        OAuthSession {
            config,
            resource: "http://127.0.0.1:3000/mcp".into(),
            // No issuer configured, so the key is the resource -- which is what
            // every `store.put` in these tests writes under.
            store_key: RwLock::new("http://127.0.0.1:3000/mcp".into()),
            token: RwLock::new(Some("stale-token".into())),
            flow: Mutex::new(flow),
            requested_scopes: RwLock::new(Vec::new()),
        }
    }

    #[tokio::test]
    async fn stale_token_is_refreshed_without_interaction() {
        let addr = spawn_token_endpoint(
            r#"{"access_token":"fresh-token","token_type":"Bearer","expires_in":3600}"#,
        )
        .await;

        let store: Arc<dyn TokenStore> = Arc::new(InMemoryTokenStore::new());
        store.put("http://127.0.0.1:3000/mcp", &stale_tokens());

        let flow = FlowState {
            client: OAuthClient::new("cid")
                .with_config(ClientConfig::new().require_https(false))
                .with_token_store(store.clone()),
            metadata: AuthorizationServerMetadata::new("http://issuer.local")
                .with_token_endpoint(format!("http://{addr}/token")),
        };
        let session = session_with(store.clone(), Some(flow));

        let token = session.refreshed_bearer().await;

        assert_eq!(token.as_deref(), Some("fresh-token"));
        let stored = store.get("http://127.0.0.1:3000/mcp").unwrap();
        assert_eq!(stored.access_token, "fresh-token");
        // No rotation in the response -- the old refresh token carries over.
        assert_eq!(stored.refresh_token.as_deref(), Some("refresh-1"));
        // The flow state survives for the next refresh.
        assert!(session.flow.lock().await.is_some());
    }

    /// A refresh response may leave `scope` out when the grant is unchanged
    /// (RFC 6749 section 5.1), and the renewed set replaces the stored one. So
    /// unless the known grant rides along, simply renewing a token forgets what
    /// it covers -- and the next step-up then widens from nothing, replacing
    /// the grant instead of adding to it.
    #[tokio::test]
    async fn a_renewal_keeps_the_grant_it_did_not_restate() {
        let addr = spawn_token_endpoint(
            r#"{"access_token":"fresh-token","token_type":"Bearer","expires_in":3600}"#,
        )
        .await;

        let store: Arc<dyn TokenStore> = Arc::new(InMemoryTokenStore::new());
        let mut restored = stale_tokens();
        restored.scope = Some("read".into());
        store.put("http://127.0.0.1:3000/mcp", &restored);

        let flow = FlowState {
            client: OAuthClient::new("cid")
                .with_config(ClientConfig::new().require_https(false))
                .with_token_store(store.clone()),
            metadata: AuthorizationServerMetadata::new("http://issuer.local")
                .with_token_endpoint(format!("http://{addr}/token")),
        };
        // Nothing recorded in memory: the state a restart leaves behind, where
        // the store is the only thing that knows what was granted.
        let session = session_with(store.clone(), Some(flow));

        assert_eq!(
            session.refreshed_bearer().await.as_deref(),
            Some("fresh-token")
        );
        assert_eq!(
            store
                .get("http://127.0.0.1:3000/mcp")
                .and_then(|tokens| tokens.scope)
                .as_deref(),
            Some("read"),
            "a renewal that restated nothing must not erase the granted scope"
        );
        assert_eq!(
            session.requested_scopes(),
            vec!["read".to_string()],
            "and a step-up must still have that grant to widen"
        );
    }

    /// The other direction: a refresh that *narrows* the grant.
    ///
    /// The in-memory set outranks the store, so a wider grant remembered from
    /// an earlier round in this process would outlive the token that carried
    /// it. A challenge demanding a scope the renewed token no longer has would
    /// then read as already covered, take the single-flight shortcut, and hand
    /// the caller that same token to be refused again on its one retry.
    #[tokio::test]
    async fn a_narrowing_renewal_is_what_the_session_remembers() {
        let addr = spawn_token_endpoint(
            r#"{"access_token":"fresh-token","token_type":"Bearer","expires_in":3600,"scope":"read"}"#,
        )
        .await;

        let store: Arc<dyn TokenStore> = Arc::new(InMemoryTokenStore::new());
        let mut restored = stale_tokens();
        restored.scope = Some("read write".into());
        store.put("http://127.0.0.1:3000/mcp", &restored);

        let flow = FlowState {
            client: OAuthClient::new("cid")
                .with_config(ClientConfig::new().require_https(false))
                .with_token_store(store.clone()),
            metadata: AuthorizationServerMetadata::new("http://issuer.local")
                .with_token_endpoint(format!("http://{addr}/token")),
        };
        let session = session_with(store.clone(), Some(flow));
        // What an earlier round in this process was granted.
        session.set_requested_scopes(vec!["read".to_string(), "write".to_string()]);

        assert_eq!(
            session.refreshed_bearer().await.as_deref(),
            Some("fresh-token")
        );
        assert_eq!(
            store
                .get("http://127.0.0.1:3000/mcp")
                .and_then(|tokens| tokens.scope)
                .as_deref(),
            Some("read"),
            "the response stated the grant, so nothing is carried over it"
        );
        assert_eq!(
            session.requested_scopes(),
            vec!["read".to_string()],
            "and the session holds what the token holds, not what it used to"
        );
    }

    #[tokio::test]
    async fn fresh_token_skips_refresh() {
        let store: Arc<dyn TokenStore> = Arc::new(InMemoryTokenStore::new());
        let mut tokens = stale_tokens();
        tokens.expires_at =
            Some(std::time::SystemTime::now() + std::time::Duration::from_secs(3600));
        store.put("http://127.0.0.1:3000/mcp", &tokens);

        // No flow state -- a refresh attempt would return None; a fresh
        // token must never get that far.
        let session = session_with(store, None);

        assert_eq!(
            session.refreshed_bearer().await.as_deref(),
            Some("stale-token")
        );
    }

    #[tokio::test]
    async fn stale_token_without_flow_state_stays_usable() {
        let store: Arc<dyn TokenStore> = Arc::new(InMemoryTokenStore::new());
        store.put("http://127.0.0.1:3000/mcp", &stale_tokens());

        let session = session_with(store, None);

        // Nothing to refresh with -- the current token is returned and
        // the 401 path decides what happens next.
        assert_eq!(
            session.refreshed_bearer().await.as_deref(),
            Some("stale-token")
        );
    }

    #[tokio::test]
    async fn session_serves_stored_unexpired_token() {
        let store = InMemoryTokenStore::new();
        store.put(
            "http://127.0.0.1:3000/mcp",
            &TokenSet {
                access_token: "stored-token".into(),
                token_type: "Bearer".into(),
                refresh_token: None,
                scope: None,
                id_token: None,
                expires_at: None,
            },
        );
        let config = OAuthClientConfig::default().with_token_store(store);
        let session = OAuthSession::new(config, "http://127.0.0.1:3000/mcp").unwrap();
        assert_eq!(session.bearer().as_deref(), Some("stored-token"));
    }

    /// A token restored from a persistent store carries a grant this process
    /// never asked for. Unless it counts as held, the first
    /// `insufficient_scope` challenge after a restart builds its step-up from
    /// the demanded scopes alone and trades away everything the restored token
    /// had -- so the next call for one of those is challenged in turn, and the
    /// two ping-pong.
    #[test]
    fn a_restored_grant_is_what_a_step_up_widens() {
        let stored = |scope: Option<&str>| {
            let store = InMemoryTokenStore::new();
            store.put(
                "http://127.0.0.1:3000/mcp",
                &TokenSet {
                    access_token: "stored-token".into(),
                    token_type: "Bearer".into(),
                    refresh_token: None,
                    scope: scope.map(str::to_owned),
                    id_token: None,
                    expires_at: None,
                },
            );
            store
        };

        // The granted scope on the stored token is the record of the grant.
        let config = OAuthClientConfig::default().with_token_store(stored(Some("read write")));
        let session = OAuthSession::new(config, "http://127.0.0.1:3000/mcp").unwrap();
        assert_eq!(
            session.requested_scopes(),
            vec!["read".to_string(), "write".to_string()],
            "a restored grant must be held, or a step-up replaces it"
        );

        // A server that granted exactly what was asked may omit `scope`
        // (RFC 6749 5.1). Configured scopes are what every flow of this session
        // requests, so they stand in.
        let config = OAuthClientConfig::default()
            .with_token_store(stored(None))
            .with_scopes(["read", "write"]);
        let session = OAuthSession::new(config, "http://127.0.0.1:3000/mcp").unwrap();
        assert_eq!(
            session.requested_scopes(),
            vec!["read".to_string(), "write".to_string()]
        );

        // Nothing stored and nothing configured: there is no grant to widen,
        // and any demanded scope is genuinely new.
        let config = OAuthClientConfig::default().with_token_store(stored(None));
        let session = OAuthSession::new(config, "http://127.0.0.1:3000/mcp").unwrap();
        assert!(session.requested_scopes().is_empty());

        // A grant narrower than the request is what the store records, and it
        // is what `requested_scopes` must report: counting a refused scope as
        // held would read the next challenge for it as an expired token rather
        // than a narrow grant, and the client would refresh into the same
        // refusal instead of widening.
        let config = OAuthClientConfig::default()
            .with_token_store(stored(Some("read")))
            .with_scopes(["read", "write"]);
        let session = OAuthSession::new(config, "http://127.0.0.1:3000/mcp").unwrap();
        assert_eq!(
            session.requested_scopes(),
            vec!["read".to_string()],
            "the granted scope outranks the configured request"
        );

        // What this process actually asked for still wins over both.
        let config = OAuthClientConfig::default()
            .with_token_store(stored(Some("read")))
            .with_scopes(["configured"]);
        let session = OAuthSession::new(config, "http://127.0.0.1:3000/mcp").unwrap();
        session.set_requested_scopes(vec!["from-this-process".to_string()]);
        assert_eq!(
            session.requested_scopes(),
            vec!["from-this-process".to_string()]
        );
    }

    /// A whole authorization server on one socket: the resource document, its
    /// own metadata, and a token endpoint that answers a refresh.
    async fn spawn_authorization_server() -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 8192];
                let read = stream.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..read]).to_string();
                let root = format!("http://{addr}");

                let body = if request.contains("/.well-known/oauth-protected-resource") {
                    format!(r#"{{"resource":"{root}/mcp","authorization_servers":["{root}"]}}"#)
                } else if request.contains("/.well-known/") {
                    format!(
                        r#"{{"issuer":"{root}","token_endpoint":"{root}/token",
                             "authorization_endpoint":"{root}/authorize",
                             "response_types_supported":["code"]}}"#
                    )
                } else {
                    r#"{"access_token":"refreshed-after-restart","token_type":"Bearer","expires_in":3600}"#.to_string()
                };

                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        });
        addr
    }

    /// An authorization server that also registers clients and answers the
    /// token endpoint *without* a `scope` -- RFC 6749 section 5.1's "you were
    /// granted exactly what you asked for", which is the case that leaves the
    /// grant to be inferred.
    async fn spawn_registering_authorization_server() -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 8192];
                let read = stream.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..read]).to_string();
                let root = format!("http://{addr}");

                let body = if request.contains("/.well-known/oauth-protected-resource") {
                    format!(r#"{{"resource":"{root}/mcp","authorization_servers":["{root}"]}}"#)
                } else if request.contains("/.well-known/") {
                    format!(
                        r#"{{"issuer":"{root}","token_endpoint":"{root}/token",
                             "authorization_endpoint":"{root}/authorize",
                             "registration_endpoint":"{root}/register",
                             "response_types_supported":["code"]}}"#
                    )
                } else if request.contains("/register") {
                    r#"{"client_id":"registered-client"}"#.to_string()
                } else {
                    r#"{"access_token":"granted-token","token_type":"Bearer","expires_in":3600}"#
                        .to_string()
                };

                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        });
        addr
    }

    /// [`spawn_registering_authorization_server`] that also advertises
    /// `client_id_metadata_document_supported`, and records every request line
    /// it served so a test can assert what the client did *not* ask for.
    async fn spawn_cimd_authorization_server()
    -> (std::net::SocketAddr, Arc<std::sync::Mutex<Vec<String>>>) {
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

                let body = if request.contains("/.well-known/oauth-protected-resource") {
                    format!(r#"{{"resource":"{root}/mcp","authorization_servers":["{root}"]}}"#)
                } else if request.contains("/.well-known/") {
                    // Registration is on offer as well: what decides the flow
                    // here is the CIMD flag, not the absence of an alternative.
                    format!(
                        r#"{{"issuer":"{root}","token_endpoint":"{root}/token",
                             "authorization_endpoint":"{root}/authorize",
                             "registration_endpoint":"{root}/register",
                             "client_id_metadata_document_supported":true,
                             "response_types_supported":["code"]}}"#
                    )
                } else {
                    r#"{"access_token":"cimd-token","token_type":"Bearer","expires_in":3600}"#
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

    /// An authorization server offering neither a registration endpoint nor
    /// client id metadata documents -- it says `false` to the latter, so there
    /// is nothing left for a client without a pre-registered id.
    async fn spawn_bare_authorization_server() -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 8192];
                let read = stream.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..read]).to_string();
                let root = format!("http://{addr}");

                let body = if request.contains("/.well-known/oauth-protected-resource") {
                    format!(r#"{{"resource":"{root}/mcp","authorization_servers":["{root}"]}}"#)
                } else {
                    format!(
                        r#"{{"issuer":"{root}","token_endpoint":"{root}/token",
                             "authorization_endpoint":"{root}/authorize",
                             "client_id_metadata_document_supported":false,
                             "response_types_supported":["code"]}}"#
                    )
                };

                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        });
        addr
    }

    /// Completes the flow without a browser by reading the `state` back off the
    /// authorization URL -- which is what the redirect would have carried.
    struct EchoesState;

    impl AuthorizationHandler for EchoesState {
        fn redirect_uri(&self) -> BoxFuture<'_, Result<String, Error>> {
            Box::pin(async { Ok("http://127.0.0.1:8919/callback".to_string()) })
        }

        fn authorize(&self, url: String) -> BoxFuture<'_, Result<CallbackParams, Error>> {
            Box::pin(async move {
                let state = url
                    .split(['?', '&'])
                    .find_map(|param| param.strip_prefix("state="))
                    .ok_or_else(|| {
                        Error::new(
                            ErrorCode::InvalidRequest,
                            "the authorization URL carried no `state`",
                        )
                    })?
                    .to_owned();
                Ok(CallbackParams {
                    code: "the-code".into(),
                    state,
                    iss: None,
                })
            })
        }
    }

    /// [`EchoesState`] that also keeps the authorization URL, so a test can
    /// read the parameters the user's browser would have carried.
    #[derive(Default)]
    struct RecordsTheUrl(std::sync::Mutex<Option<String>>);

    impl AuthorizationHandler for Arc<RecordsTheUrl> {
        fn redirect_uri(&self) -> BoxFuture<'_, Result<String, Error>> {
            Box::pin(async { Ok("http://127.0.0.1:8919/callback".to_string()) })
        }

        fn authorize(&self, url: String) -> BoxFuture<'_, Result<CallbackParams, Error>> {
            if let Ok(mut seen) = self.0.lock() {
                *seen = Some(url.clone());
            }
            EchoesState.authorize(url)
        }
    }

    /// A Client ID Metadata Document needs no registration: the URL *is* the
    /// id, and the server resolves it. So the flow completes with the URL on
    /// the authorization request and never touches the registration endpoint
    /// -- which this server offers, to show it is the advertised CIMD support
    /// and not the lack of an alternative that decided it.
    #[tokio::test]
    async fn a_cimd_client_authorizes_without_registering() {
        let (addr, seen) = spawn_cimd_authorization_server().await;
        let resource = format!("http://{addr}/mcp");

        let handler = Arc::new(RecordsTheUrl::default());
        let config = OAuthClientConfig::default()
            .require_https(false)
            .with_client_id_document(CIMD_URL)
            .with_handler(handler.clone());
        let session = OAuthSession::new(config, &resource).unwrap();

        let token = session.authorize(None, None).await.expect("the flow runs");
        assert_eq!(&*token, "cimd-token");

        let url = handler.0.lock().unwrap().clone().expect("a URL was built");
        assert!(
            url.contains("client_id=https%3A%2F%2Fapp.example.com%2Fmcp-client.json"),
            "the document URL is what identifies the client: {url}"
        );

        let requests = seen.lock().unwrap().clone();
        assert!(
            !requests.iter().any(|req| req.contains("/register")),
            "a CIMD client has nothing to register: {requests:?}"
        );
    }

    /// A document identity is portable, so a CIMD client whose resource has
    /// moved completes its flow against a server its configuration does not
    /// name. What it must not do is file that server's tokens under the
    /// configured issuer: the label would be a lie, and if the resource ever
    /// moved back, the configured key would hand the old server a refresh
    /// token the new one minted -- the leak the keying exists to stop.
    #[tokio::test]
    async fn a_portable_client_files_tokens_under_the_server_that_minted_them() {
        let (addr, _) = spawn_cimd_authorization_server().await;
        let resource = format!("http://{addr}/mcp");
        let stale_config = "https://old-auth.example.com";

        let store: Arc<dyn TokenStore> = Arc::new(InMemoryTokenStore::new());
        let config = OAuthClientConfig {
            store: store.clone(),
            ..OAuthClientConfig::default()
                .require_https(false)
                .with_client_id_document(CIMD_URL)
                // Names the server this resource used to use. A portable
                // identity is exempt from the mismatch check, so the flow runs.
                .with_issuer(stale_config)
                .with_handler(EchoesState)
        };
        let session = OAuthSession::new(config, &resource).unwrap();

        let token = session.authorize(None, None).await.expect("the flow runs");
        assert_eq!(&*token, "cimd-token");

        assert!(
            store.get(&format!("{stale_config}|{resource}")).is_none(),
            "nothing may be filed under a server that minted none of it"
        );
        assert_eq!(
            store
                .get(&format!("http://{addr}|{resource}"))
                .map(|tokens| tokens.access_token),
            Some("cimd-token".to_owned()),
            "the tokens belong to the server the flow actually ran against"
        );
        assert_eq!(
            &*session.store_key(),
            format!("http://{addr}|{resource}"),
            "and the session follows them there, so its staleness probe is not \
             left watching an empty slot"
        );
    }

    /// The restart after that migration. The stale configuration still names
    /// the old server, but the tokens are filed under the one that minted
    /// them, and that is the slot the flow renews from -- so a portable
    /// identity does not pay for consent again on every start.
    #[tokio::test]
    async fn a_migrated_portable_identity_renews_after_a_restart() {
        let (addr, seen) = spawn_cimd_authorization_server().await;
        let resource = format!("http://{addr}/mcp");

        let store: Arc<dyn TokenStore> = Arc::new(InMemoryTokenStore::new());
        store.put(&format!("http://{addr}|{resource}"), &stale_tokens());

        let config = OAuthClientConfig {
            store,
            ..OAuthClientConfig::default()
                .require_https(false)
                .with_client_id_document(CIMD_URL)
                .with_issuer("https://old-auth.example.com")
                .with_handler(NoInteraction)
        };
        let session = OAuthSession::new(config, &resource).unwrap();

        let token = session
            .authorize(None, Some("the-expired-token"))
            .await
            .expect("the new server's own stored token is what answers this");
        assert_eq!(&*token, "cimd-token");

        let requests = seen.lock().unwrap().clone();
        assert!(
            requests.iter().any(|req| req.contains("refresh_token")),
            "renewed rather than re-authorized: {requests:?}"
        );
    }

    /// A grant the token response did not restate is inferred from the request
    /// -- and has to be written down where a restart can find it.
    ///
    /// Omitting `scope` is how a server says "exactly what you asked for", so
    /// this is the ordinary case rather than an edge one. Recorded in memory
    /// alone it dies with the process, and the next run's first
    /// `insufficient_scope` challenge widens from nothing: the step-up asks for
    /// the demanded scope by itself and trades away everything the token
    /// already carried.
    #[tokio::test]
    async fn an_inferred_grant_is_stored_where_a_restart_can_find_it() {
        let addr = spawn_registering_authorization_server().await;
        let resource = format!("http://{addr}/mcp");

        let store: Arc<dyn TokenStore> = Arc::new(InMemoryTokenStore::new());
        let config = OAuthClientConfig {
            store: store.clone(),
            ..OAuthClientConfig::default()
        }
        .require_https(false)
        .with_handler(EchoesState);
        // No configured scopes: the challenge is what the flow asks for, and
        // the store is then the only place that grant can be kept.
        let session = OAuthSession::new(config, &resource).unwrap();

        let token = session
            .authorize(
                Some(r#"Bearer error="insufficient_scope", scope="admin""#),
                None,
            )
            .await
            .expect("the flow completes");
        assert_eq!(&*token, "granted-token");

        assert_eq!(
            store
                .get(&resource)
                .and_then(|tokens| tokens.scope)
                .as_deref(),
            Some("admin"),
            "a grant the response left implicit must still be written down"
        );

        // What the next process sees: a fresh session over the same store, with
        // nothing in memory.
        let restarted = OAuthSession::new(
            OAuthClientConfig {
                store,
                ..OAuthClientConfig::default()
            },
            &resource,
        )
        .unwrap();
        assert_eq!(
            restarted.requested_scopes(),
            vec!["admin".to_string()],
            "and be there for the next step-up to widen"
        );
    }

    /// A handler that supplies a redirect URI but refuses to interact, so a
    /// flow that should never have reached the user says so instead of opening
    /// a browser and waiting five minutes.
    struct NoInteraction;

    impl AuthorizationHandler for NoInteraction {
        fn redirect_uri(&self) -> BoxFuture<'_, Result<String, Error>> {
            Box::pin(async { Ok("http://127.0.0.1:8919/callback".to_string()) })
        }

        fn authorize(&self, _url: String) -> BoxFuture<'_, Result<CallbackParams, Error>> {
            Box::pin(async {
                Err(Error::new(
                    ErrorCode::InvalidRequest,
                    "the stored refresh token should have been used instead",
                ))
            })
        }
    }

    /// A durable token store outlives the process; the flow state that knows
    /// how to use it does not. After a restart the refresh token in that store
    /// is still good, and spending it is the difference between a silent
    /// renewal and walking the user through consent again.
    ///
    /// It takes a named issuer, because that is what the entry is filed under
    /// -- see [`an_unbound_refresh_token_is_not_offered_after_a_restart`] and
    /// [`a_refresh_token_does_not_follow_the_resource_to_a_new_issuer`].
    #[tokio::test]
    async fn a_stored_refresh_token_survives_a_restart() {
        let addr = spawn_authorization_server().await;
        let resource = format!("http://{addr}/mcp");

        let store: Arc<dyn TokenStore> = Arc::new(InMemoryTokenStore::new());
        // Under the issuer that minted it, which is where the next run looks.
        store.put(&format!("http://{addr}|{resource}"), &stale_tokens());

        // A fresh process: a store with a usable refresh token, and no flow
        // state at all.
        let config = OAuthClientConfig {
            store: store.clone(),
            ..OAuthClientConfig::default()
                .require_https(false)
                .with_client_id("cid")
                .with_issuer(format!("http://{addr}"))
                .with_handler(NoInteraction)
        };
        let session = OAuthSession::new(config, &resource).unwrap();
        assert!(
            session.flow.lock().await.is_none(),
            "a restart starts with nothing cached"
        );

        let token = session
            .authorize(None, Some("the-expired-token"))
            .await
            .expect("the stored refresh token is what answers this");

        assert_eq!(&*token, "refreshed-after-restart");
        assert!(
            session.flow.lock().await.is_some(),
            "and what made it work is kept, so the next refresh is the cheap path"
        );
    }

    /// The same restart, with nothing saying which authorization server minted
    /// the stored token. A refresh token is a bearer credential for the
    /// endpoint that issued it, and the server this flow discovered is only
    /// vouched for by the resource -- which is precisely what an attacker who
    /// controls the resource would rewrite. So the token stays in the store
    /// and the user is asked instead.
    #[tokio::test]
    async fn an_unbound_refresh_token_is_not_offered_after_a_restart() {
        let addr = spawn_authorization_server().await;
        let resource = format!("http://{addr}/mcp");

        let store: Arc<dyn TokenStore> = Arc::new(InMemoryTokenStore::new());
        store.put(&resource, &stale_tokens());

        let config = OAuthClientConfig {
            store,
            ..OAuthClientConfig::default()
                .require_https(false)
                .with_client_id("cid")
                .with_handler(NoInteraction)
        };
        let session = OAuthSession::new(config, &resource).unwrap();

        // `NoInteraction` fails rather than opening a browser, so reaching the
        // interactive step is what this asserts.
        let err = session
            .authorize(None, Some("the-expired-token"))
            .await
            .expect_err("an unbound refresh token must not be spent");
        assert!(
            err.to_string().contains("should have been used instead",),
            "the flow must reach the interactive step, not fail earlier: {err}"
        );
    }

    /// The migration case, and the one a configured issuer alone does not
    /// cover: the store holds a refresh token from the *old* authorization
    /// server, and the operator has since pointed `with_issuer` at the new one
    /// -- which is exactly what migrating means. Checking the configuration
    /// against the discovered issuer then says "these match" about a token
    /// neither of them minted, and the old server's credential goes to the new
    /// server.
    ///
    /// What stops it is where the token is filed: under the issuer that minted
    /// it, so the new issuer's key finds nothing and the user is asked instead.
    #[tokio::test]
    async fn a_refresh_token_does_not_follow_the_resource_to_a_new_issuer() {
        let addr = spawn_authorization_server().await;
        let resource = format!("http://{addr}/mcp");

        // Everything the previous deployment left behind, filed under the
        // server that issued it.
        let store: Arc<dyn TokenStore> = Arc::new(InMemoryTokenStore::new());
        let previous_issuer = "https://old-auth.example.com";
        store.put(&format!("{previous_issuer}|{resource}"), &stale_tokens());

        // And the configuration as it reads after the migration: the new
        // issuer, which is also the one discovery returns.
        let config = OAuthClientConfig {
            store: store.clone(),
            ..OAuthClientConfig::default()
                .require_https(false)
                .with_client_id("cid")
                .with_issuer(format!("http://{addr}"))
                .with_handler(NoInteraction)
        };
        let session = OAuthSession::new(config, &resource).unwrap();

        let err = session
            .authorize(None, Some("the-expired-token"))
            .await
            .expect_err("the old server's refresh token must not be spent at the new one");
        assert!(
            err.to_string().contains("should have been used instead"),
            "the flow must reach the interactive step, not fail earlier: {err}"
        );

        // Untouched, rather than renewed into the new server's tokens.
        assert_eq!(
            store
                .get(&format!("{previous_issuer}|{resource}"))
                .map(|tokens| tokens.access_token),
            Some("stale-token".to_owned()),
            "the old entry must be left exactly where it was"
        );
    }

    /// Two callers refused for the same missing scope must not walk the user
    /// through consent twice.
    ///
    /// The loser of the single-flight lock arrives after the winner has
    /// recorded the widened grant and stored its token. Forcing the step-up on
    /// the error code alone would send it straight past that and into a second
    /// interactive flow, for a scope it now already holds.
    #[tokio::test]
    async fn the_loser_of_a_step_up_takes_the_winners_token() {
        let store = InMemoryTokenStore::new();
        store.put(
            "http://127.0.0.1:9/mcp",
            &TokenSet {
                access_token: "widened-token".into(),
                token_type: "Bearer".into(),
                refresh_token: None,
                // What the winner was granted, which covers the challenge.
                scope: Some("admin".into()),
                id_token: None,
                expires_at: None,
            },
        );

        let config = OAuthClientConfig::default()
            .require_https(false)
            .with_token_store(store);
        let session = OAuthSession::new(config, "http://127.0.0.1:9/mcp").unwrap();

        // Nothing listens on port 9, so a run that reaches discovery fails on
        // connect rather than hanging -- the shortcut is what keeps it away
        // from the network at all.
        let token = session
            .authorize(
                Some(r#"Bearer error="insufficient_scope", scope="admin""#),
                Some("the-refused-token"),
            )
            .await
            .expect("the grant on record already covers the challenge");

        assert_eq!(
            &*token, "widened-token",
            "the loser must reuse what the winner obtained"
        );
    }

    /// A step-up that named no scope cannot be satisfied by a token that merely
    /// changed.
    ///
    /// `scope` is optional in RFC 6750, so a server may say the grant is too
    /// narrow without saying what it wants. There is then nothing to check
    /// coverage against -- and a rotated token is no substitute, because a
    /// refresh renews a grant without widening it. Handing it back would be the
    /// refresh path wearing the step-up's clothes, and the caller would spend
    /// its one retry on credentials short by exactly as much as before.
    #[tokio::test]
    async fn a_scope_less_step_up_is_not_satisfied_by_a_rotated_token() {
        // Nothing listens on port 9, so a run that reaches discovery fails on
        // connect: reaching the network at all is the assertion.
        const RESOURCE: &str = "http://127.0.0.1:9/mcp";

        let store = InMemoryTokenStore::new();
        store.put(
            RESOURCE,
            &TokenSet {
                // What another request's refresh left behind: a different token,
                // covering exactly what the old one did.
                access_token: "rotated-token".into(),
                token_type: "Bearer".into(),
                refresh_token: None,
                scope: Some("read".into()),
                id_token: None,
                expires_at: None,
            },
        );

        let config = OAuthClientConfig::default()
            .require_https(false)
            .with_token_store(store);
        let session = OAuthSession::new(config, RESOURCE).unwrap();

        let err = session
            .authorize(
                Some(r#"Bearer error="insufficient_scope""#),
                Some("the-refused-token"),
            )
            .await
            .expect_err("a rotated token is not evidence of a wider grant");
        assert!(
            !err.to_string().contains("rotated-token"),
            "the flow must be run, not short-circuited: {err}"
        );

        // The named case is the one the shortcut exists for, and it still
        // works: the grant on record covers what the challenge demanded.
        let store = InMemoryTokenStore::new();
        store.put(
            RESOURCE,
            &TokenSet {
                access_token: "widened-token".into(),
                token_type: "Bearer".into(),
                refresh_token: None,
                scope: Some("admin".into()),
                id_token: None,
                expires_at: None,
            },
        );
        let config = OAuthClientConfig::default()
            .require_https(false)
            .with_token_store(store);
        let session = OAuthSession::new(config, RESOURCE).unwrap();

        let token = session
            .authorize(
                Some(r#"Bearer error="insufficient_scope", scope="admin""#),
                Some("the-refused-token"),
            )
            .await
            .expect("a demand the grant on record covers");
        assert_eq!(&*token, "widened-token");
    }

    /// A configured scope set is a ceiling as well as a floor: the flow asks
    /// for exactly it. So a challenge demanding something outside it describes
    /// a grant this client cannot obtain, and running the flow would interrupt
    /// the user for consent only to come back without the scope that was
    /// missing -- the retry then fails identically.
    #[tokio::test]
    async fn a_demand_outside_the_configured_scopes_ends_the_call() {
        // Plain HTTP, which discovery refuses before opening a socket, so this
        // test touches no network whichever way the guard goes.
        const RESOURCE: &str = "http://127.0.0.1:9/mcp";

        let config = OAuthClientConfig::default().with_scopes(["read"]);
        let session = OAuthSession::new(config, RESOURCE).unwrap();

        let err = session
            .authorize(
                Some(r#"Bearer error="insufficient_scope", scope="admin""#),
                None,
            )
            .await
            .expect_err("a scope this client may not request cannot be obtained");
        let msg = err.to_string();
        assert!(
            msg.contains("admin") && msg.contains("with_scopes"),
            "the error must name the scope and how to allow it, got: {msg}"
        );

        // A demand the configured set already covers is not this case: it is
        // an ordinary re-authorization and proceeds to discovery, which is
        // where this test leaves it.
        let config = OAuthClientConfig::default().with_scopes(["read", "admin"]);
        let session = OAuthSession::new(config, RESOURCE).unwrap();

        let err = session
            .authorize(
                Some(r#"Bearer error="insufficient_scope", scope="admin""#),
                None,
            )
            .await
            .expect_err("the resource is unreachable, so the flow cannot finish");
        assert!(
            !err.to_string().contains("with_scopes"),
            "a covered demand must not be refused up front, got: {err}"
        );
    }
}


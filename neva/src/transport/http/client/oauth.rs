//! Client-side OAuth 2.1 authorization for the Streamable HTTP transport.
//!
//! Implements the MCP authorization sequence on top of
//! [`volga-oauth-client`](https://docs.rs/volga-oauth-client) (framework
//! independent — plain hyper): a `401` challenge is parsed for its
//! `resource_metadata` pointer (RFC 9728 §5.1), the Protected Resource
//! Metadata and the authorization server metadata are discovered
//! (RFC 8414, OIDC fallback), the client registers dynamically when no
//! `client_id` is configured (RFC 7591, `application_type: "native"` for
//! loopback redirects), and the authorization-code + PKCE flow runs with
//! the server's canonical URI as the RFC 8707 resource indicator. The
//! callback is checked for `state` and the RFC 9207 `iss` parameter
//! before the code is exchanged.
//!
//! The interactive step is pluggable through [`AuthorizationHandler`];
//! the default [`LoopbackHandler`] serves desktop/CLI clients by opening
//! the system browser and capturing the redirect on a loopback listener.

use futures_util::future::BoxFuture;
use std::sync::{Arc, RwLock};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Mutex,
};

use crate::error::{Error, ErrorCode};

use volga_oauth_client::{
    AuthorizationServerMetadata, BearerChallenge, ClientConfig, ClientError, ClientMetadata,
    DiscoveryClient, OAuthClient, RegistrationClient, canonicalize_resource_uri,
    protected_resource_metadata_url,
};
pub use volga_oauth_client::{InMemoryTokenStore, TokenSet, TokenStore};

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
    /// (RFC 6749 §4.1.2.1) or is missing `code`/`state`.
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

/// Minimal `application/x-www-form-urlencoded` pair iterator — enough
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
/// # Example
/// ```no_run
/// use futures_util::future::BoxFuture;
/// use neva::auth::oauth::{AuthorizationHandler, CallbackParams};
/// use neva::error::Error;
///
/// struct MyUi;
///
/// impl AuthorizationHandler for MyUi {
///     fn redirect_uri(&self) -> BoxFuture<'_, Result<String, Error>> {
///         Box::pin(async { Ok("https://my.app/oauth/callback".into()) })
///     }
///     fn authorize(&self, url: String) -> BoxFuture<'_, Result<CallbackParams, Error>> {
///         Box::pin(async move {
///             // show `url` to the user, await the callback…
///             # let _ = url;
///             todo!()
///         })
///     }
/// }
/// ```
pub trait AuthorizationHandler: Send + Sync + 'static {
    /// The redirect URI the authorization response will be delivered to.
    ///
    /// Called once per flow, before dynamic client registration — the
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
/// The redirect URI is `http://127.0.0.1:{port}/callback` — loopback
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

    /// Pins the callback listener to a fixed port — required when the
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
/// (`GET /callback?code=…&state=… HTTP/1.1`) and parses it.
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

/// Launches the system browser at `url`, best-effort — on failure the
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
/// Everything is optional: without a `client_id` the client registers
/// dynamically (RFC 7591); without scopes the resource's advertised
/// `scopes_supported` are requested; tokens live in an in-process store
/// and the interactive step runs through [`LoopbackHandler`] unless
/// replaced.
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
    scopes: Option<Vec<String>>,
    require_https: bool,
    store: Arc<dyn TokenStore>,
    handler: Arc<dyn AuthorizationHandler>,
}

impl std::fmt::Debug for OAuthClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthClientConfig")
            .field("client_id", &self.client_id)
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
            scopes: None,
            require_https: true,
            store: Arc::new(InMemoryTokenStore::new()),
            handler: Arc::new(LoopbackHandler::new()),
        }
    }
}

impl OAuthClientConfig {
    /// Uses a pre-registered OAuth client id instead of dynamic
    /// registration.
    pub fn with_client_id(mut self, client_id: impl Into<String>) -> Self {
        self.client_id = Some(client_id.into());
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
    /// [`TokenStore`] (encrypted file, OS keychain, …).
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

    fn client_config(&self) -> ClientConfig {
        ClientConfig::new().require_https(self.require_https)
    }
}

/// Per-connection OAuth state: the current access token and the
/// single-flight authorization flow.
pub(crate) struct OAuthSession {
    config: OAuthClientConfig,
    /// Canonicalized server URL — the RFC 8707 resource indicator and
    /// the token-store key.
    resource: String,
    /// Current bearer token, read on every outgoing request.
    token: RwLock<Option<Arc<str>>>,
    /// Serializes authorization flows: concurrent 401s run one flow.
    flow: Mutex<()>,
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
        let resource = canonicalize_resource_uri(server_url)
            .map_err(|err| Error::new(ErrorCode::InternalError, err.to_string()))?;
        let token = config
            .store
            .get(&resource)
            .filter(|tokens| !tokens.is_expired())
            .map(|tokens| tokens.access_token.into());
        Ok(Self {
            config,
            resource,
            token: RwLock::new(token),
            flow: Mutex::new(()),
        })
    }

    /// The current bearer token, if any.
    pub(crate) fn bearer(&self) -> Option<Arc<str>> {
        self.token
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Runs the authorization flow triggered by a `401` and returns the
    /// fresh bearer token.
    ///
    /// `www_authenticate` is the challenge header value, when present —
    /// its `resource_metadata` pointer takes precedence over well-known
    /// derivation. `used` is the token the failed request carried:
    /// concurrent callers that lost the race simply pick up the token
    /// the winning flow produced.
    pub(crate) async fn authorize(
        &self,
        www_authenticate: Option<&str>,
        used: Option<&str>,
    ) -> Result<Arc<str>, Error> {
        let _flight = self.flow.lock().await;

        // Someone else completed the flow while this caller waited.
        if let Some(current) = self.bearer()
            && used != Some(&*current)
        {
            return Ok(current);
        }

        let metadata_url = www_authenticate
            .and_then(|header| BearerChallenge::parse(header).ok())
            .and_then(|challenge| challenge.resource_metadata().map(str::to_owned));
        let metadata_url = match metadata_url {
            Some(url) => url,
            None => protected_resource_metadata_url(&self.resource)
                .map_err(|err| Error::new(ErrorCode::InternalError, err.to_string()))?,
        };

        let discovery = DiscoveryClient::with_config(self.config.client_config());
        let resource_metadata = discovery
            .fetch_resource_metadata_from_url(&metadata_url, Some(&self.resource))
            .await
            .map_err(flow_error)?;
        let server_metadata = discovery
            .discover_authorization_server(&resource_metadata)
            .await
            .map_err(flow_error)?;

        let redirect_uri = self.config.handler.redirect_uri().await?;
        let client = self.build_client(&server_metadata, &redirect_uri).await?;

        let scopes = self
            .config
            .scopes
            .clone()
            .unwrap_or_else(|| resource_metadata.scopes_supported.clone());
        let request = client
            .authorization_request(&server_metadata)
            .with_scopes(scopes)
            .with_resource(self.resource.clone())
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

        let tokens = exchange_code(client, server_metadata, params, request).await?;

        self.config.store.put(&self.resource, &tokens);
        let token: Arc<str> = tokens.access_token.into();
        *self
            .token
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(token.clone());
        Ok(token)
    }

    /// Builds the [`OAuthClient`] — from the configured `client_id` or
    /// through dynamic registration (RFC 7591).
    async fn build_client(
        &self,
        server_metadata: &AuthorizationServerMetadata,
        redirect_uri: &str,
    ) -> Result<OAuthClient, Error> {
        let client = match &self.config.client_id {
            Some(client_id) => {
                let mut client = OAuthClient::new(client_id.clone());
                if let Some(secret) = &self.config.client_secret {
                    client = client.with_secret(secret.clone());
                }
                client
            }
            None => {
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
///
/// A loopback redirect URI makes this a **native** client
/// (`application_type: "native"`) — authorization servers reject `web`
/// clients with plain-http loopback redirects, which is exactly the
/// desktop/CLI case.
fn registration_metadata(redirect_uri: &str) -> ClientMetadata {
    let mut metadata = ClientMetadata::default()
        .with_redirect_uris([redirect_uri])
        .with_grant_types(["authorization_code", "refresh_token"])
        .with_response_types(["code"])
        .with_token_endpoint_auth_method("none")
        .with_client_name(DEFAULT_CLIENT_NAME);
    if is_loopback_redirect(redirect_uri) {
        metadata = metadata.with_additional_field("application_type", "native");
    }
    metadata
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
    // Bracketed IPv6 hosts carry colons of their own — split on the
    // closing bracket first, then strip a `:port` for everything else.
    let host = match authority.split_once(']') {
        Some((bracketed, _)) => &authority[..bracketed.len() + 1],
        None => authority
            .rsplit_once(':')
            .map_or(authority, |(host, _port)| host),
    };
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

/// Validates the RFC 9207 `iss` authorization-response parameter.
///
/// When the server metadata advertises
/// `authorization_response_iss_parameter_supported`, the parameter is
/// required and must match the issuer; when it is merely present, it
/// must still match. A mismatch means the response may come from a
/// different (potentially malicious) authorization server — mix-up
/// attack — and aborts the flow.
fn validate_issuer(
    params: &CallbackParams,
    metadata: &AuthorizationServerMetadata,
) -> Result<(), Error> {
    let supported = metadata
        .additional_fields
        .get("authorization_response_iss_parameter_supported")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

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

/// Exchanges the authorization code for tokens on a dedicated
/// current-thread runtime.
///
/// `OAuthClient::exchange_code` in volga-oauth-client 0.9.5 holds a
/// `form_urlencoded::Serializer` across an `.await`, which makes its
/// future `!Send` — it cannot run inside neva's spawned request tasks
/// directly. The token exchange is a single short round-trip at the end
/// of an interactive flow, so bridging it through `spawn_blocking` is
/// invisible in practice. Drop this bridge once upstream releases a
/// `Send` `exchange_code`.
async fn exchange_code(
    client: OAuthClient,
    server_metadata: AuthorizationServerMetadata,
    params: CallbackParams,
    request: volga_oauth_client::AuthorizationRequest,
) -> Result<TokenSet, Error> {
    tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(Error::from)?;
        rt.block_on(client.exchange_code(&server_metadata, &params.code, &request))
            .map_err(flow_error)
    })
    .await
    .map_err(|err| Error::new(ErrorCode::InternalError, err.to_string()))?
}

/// Maps a `volga-oauth-client` failure onto neva's error type.
fn flow_error(err: ClientError) -> Error {
    Error::new(
        ErrorCode::InternalError,
        format!("OAuth flow failed: {err}"),
    )
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
        assert_eq!(
            metadata.additional_fields.get("application_type"),
            Some(&serde_json::Value::from("native"))
        );
        assert_eq!(metadata.token_endpoint_auth_method.as_deref(), Some("none"));
    }

    #[test]
    fn web_registration_stays_a_web_client() {
        let metadata = registration_metadata("https://my.app/oauth/callback");
        assert!(!metadata.additional_fields.contains_key("application_type"));
    }

    fn as_metadata(supported: Option<bool>) -> AuthorizationServerMetadata {
        let mut metadata = AuthorizationServerMetadata::new("https://auth.example.com");
        if let Some(supported) = supported {
            metadata = metadata
                .with_additional_field("authorization_response_iss_parameter_supported", supported);
        }
        metadata
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
}

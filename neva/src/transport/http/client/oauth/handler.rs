//! The interactive step of the authorization-code flow.
//!
//! Everything between "the client has an authorization URL" and "the client has
//! a `code`" happens outside the protocol, in whatever the embedder calls a user
//! interface. [`AuthorizationHandler`] is that seam;
//! [`LoopbackHandler`] is the desktop/CLI answer to it -- open the system
//! browser, listen on a loopback port, take the callback off the first request
//! that arrives.

use super::*;

/// Default time the [`LoopbackHandler`] waits for the user to complete
/// authorization in the browser.
const DEFAULT_AUTH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

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

        for (key, value) in form_urlencoded::parse(query.as_bytes()) {
            match key.as_ref() {
                "code" => code = Some(value.into_owned()),
                "state" => state = Some(value.into_owned()),
                "iss" => iss = Some(value.into_owned()),
                "error" => error = Some(value.into_owned()),
                "error_description" => error_description = Some(value.into_owned()),
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

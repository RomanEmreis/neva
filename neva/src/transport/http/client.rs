//! HTTP client implementation

use self::mcp_session::McpSession;
use crate::{
    error::{Error, ErrorCode},
    transport::http::{ClientRuntimeContext, MCP_SESSION_ID, get_mcp_session_id},
    types::Message,
};
use futures_util::{StreamExt, TryStreamExt};
use reqwest::header::{CACHE_CONTROL, HeaderName};
use reqwest::{
    RequestBuilder,
    header::{ACCEPT, CONTENT_TYPE},
};
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "client-tls")]
use tls_config::ClientTlsConfig;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub(super) mod mcp_session;
#[cfg(feature = "client-oauth")]
pub(crate) mod oauth;
#[cfg(feature = "client-tls")]
pub(crate) mod tls_config;

/// How outgoing requests are authenticated.
///
/// Built once per connection from the runtime context; cheap to clone
/// into every request task.
#[derive(Clone)]
enum ClientAuth {
    /// No credential attached.
    None,
    /// Static bearer token from `HttpClient::with_auth`.
    Static(Arc<str>),
    /// Managed OAuth session -- the token changes as flows complete.
    #[cfg(feature = "client-oauth")]
    OAuth(Arc<oauth::OAuthSession>),
}

impl ClientAuth {
    /// The credential to attach to the next request, if any. Under a
    /// managed OAuth session a token about to expire is refreshed first
    /// (non-interactive, when a refresh token is available).
    async fn fresh_credential(&self) -> Option<Credential> {
        match self {
            ClientAuth::None => None,
            ClientAuth::Static(token) => Some(Credential::Bearer(token.clone())),
            #[cfg(feature = "client-oauth")]
            ClientAuth::OAuth(session) => session.refreshed_credential().await,
        }
    }

    fn from_static(access_token: Option<Box<[u8]>>) -> Self {
        match access_token {
            Some(token) => ClientAuth::Static(String::from_utf8_lossy(&token).into()),
            None => ClientAuth::None,
        }
    }
}

/// What one request presents to prove it may be made.
///
/// A bearer token is the whole credential: the same header value serves every
/// request, and holding it is what authorizes them. A DPoP-bound token is only
/// half of one (RFC 9449) -- the other half is a proof signed over the method
/// and URL of *this* request and over the token itself, so it cannot be
/// prepared once per connection the way a bearer token can. That is why this
/// is a credential rather than a header value, and why attaching it is a
/// fallible step taken per request.
#[derive(Clone)]
pub(crate) enum Credential {
    /// `Authorization: Bearer <token>` -- RFC 6750.
    Bearer(Arc<str>),
    /// `Authorization: DPoP <token>` plus a freshly signed `DPoP` proof --
    /// RFC 9449 sections 4 and 7.1.
    #[cfg(feature = "client-oauth-dpop")]
    Dpop {
        /// The bound token set. Carried whole because the proof binds to
        /// `dpop_jkt`, and presenting a token under the wrong key is refused
        /// by the resource on every request.
        tokens: Arc<oauth::TokenSet>,
        /// The key the tokens are bound to, shared with the OAuth session so
        /// both see the same nonces.
        key: oauth::Dpop,
    },
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // An access token is a credential whatever binds it, so what is shown
        // is the scheme and -- for a bound one -- the key it names, which is a
        // public value and the one thing worth telling two of these apart by.
        match self {
            Self::Bearer(_) => f.debug_struct("Bearer").finish_non_exhaustive(),
            #[cfg(feature = "client-oauth-dpop")]
            Self::Dpop { key, .. } => f
                .debug_struct("Dpop")
                .field("jkt", &key.thumbprint())
                .finish_non_exhaustive(),
        }
    }
}

impl Credential {
    /// The access token itself -- what a caller compares to decide whether a
    /// flow produced something new.
    pub(crate) fn access_token(&self) -> &str {
        match self {
            Self::Bearer(token) => token,
            #[cfg(feature = "client-oauth-dpop")]
            Self::Dpop { tokens, .. } => &tokens.access_token,
        }
    }

    /// Puts this credential on `req`, which is about to be sent as `method`
    /// to `url`.
    ///
    /// Returns the DPoP nonce the proof carried, if any: a `use_dpop_nonce`
    /// refusal has to be compared against what *this* request sent rather than
    /// against the shared nonce state, which a concurrent request to the same
    /// origin may have moved on in the meantime (RFC 9449 section 8).
    ///
    /// `nonce` forces the proof to carry exactly that value, which is what the
    /// retry of a refused request does.
    #[cfg_attr(not(feature = "client-oauth-dpop"), allow(unused_variables))]
    fn attach(
        &self,
        req: RequestBuilder,
        method: &reqwest::Method,
        url: &str,
        nonce: Option<&str>,
    ) -> Result<(RequestBuilder, Option<String>), Error> {
        match self {
            Self::Bearer(token) => Ok((req.bearer_auth(token), None)),
            #[cfg(feature = "client-oauth-dpop")]
            Self::Dpop { tokens, key } => {
                let mut headers = reqwest::header::HeaderMap::new();
                let sent = match nonce {
                    Some(nonce) => {
                        key.authorize_with_nonce(&mut headers, method, url, tokens, nonce)
                    }
                    None => key.authorize(&mut headers, method, url, tokens),
                }
                .map_err(|err| Error::new(ErrorCode::InvalidRequest, err.to_string()))?;

                Ok((req.headers(headers), sent))
            }
        }
    }
}

/// Sends the request `build` produces with `credential` attached, answering a
/// `use_dpop_nonce` refusal with the one retry it asks for.
///
/// The request is built rather than handed over because it may go out twice,
/// and the second attempt needs a fresh proof: a DPoP proof covers one request,
/// carries a one-shot `jti`, and a resource that remembers them refuses a
/// replay (RFC 9449 section 4.3).
///
/// That retry is not an authorization: the token is good and so is the key.
/// The server has simply not handed out its nonce yet, which it cannot do
/// before being asked -- so the first request of a session to a nonce-taking
/// resource is expected to be refused exactly once, and answering it here
/// keeps it from being read as a credential problem and spending the caller's
/// one re-authorization on it.
#[cfg_attr(not(feature = "client-oauth-dpop"), allow(unused_variables))]
async fn send_authorized(
    credential: Option<&Credential>,
    method: &reqwest::Method,
    url: &str,
    build: impl Fn() -> RequestBuilder,
) -> Result<reqwest::Response, Error> {
    let transport = |err: reqwest::Error| Error::new(ErrorCode::InternalError, err.to_string());

    let Some(credential) = credential else {
        return build().send().await.map_err(transport);
    };

    let (request, nonce_sent) = credential.attach(build(), method, url, None)?;
    #[cfg(not(feature = "client-oauth-dpop"))]
    let _ = nonce_sent;

    let resp = request.send().await.map_err(transport)?;

    #[cfg(feature = "client-oauth-dpop")]
    if let Credential::Dpop { key, .. } = credential {
        // Worth doing on every response, not only a refusal: a server may
        // supply the nonce alongside an answer it was willing to give, and
        // adopting it there spares the next request the round trip of being
        // told. What comes back is what *this* response demanded, which is
        // what a retry has to answer with -- the shared state may have moved
        // on if another request to the same origin was in flight.
        let demanded = key.accept_nonce(url, resp.headers());

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED
            && let Some(demanded) = demanded
            && use_dpop_nonce(resp.headers())
            && Some(demanded.as_str()) != nonce_sent.as_deref()
        {
            let (retried, _) = credential.attach(build(), method, url, Some(&demanded))?;
            return retried.send().await.map_err(transport);
        }
    }

    Ok(resp)
}

// SSE constants -- the standalone GET stream serves legacy peers only;
// its machinery compiles under both flags for the dual-mode client and
// activates at runtime when a legacy `initialize` handshake happens.
mod headers;
#[cfg(not(feature = "legacy-spec"))]
mod listen;
mod send;
mod sse;

use headers::*;
#[cfg(not(feature = "legacy-spec"))]
use listen::*;
use send::*;
use sse::*;

const LAST_EVENT_ID: HeaderName = HeaderName::from_static("last-event-id");
const SSE_RECONNECT_DELAY: Duration = Duration::from_secs(3);
const STREAM_ENDED_BEFORE_RESPONSE: &str = "POST SSE stream ended before the response arrived";

pub(super) async fn connect(rt: ClientRuntimeContext, token: CancellationToken) {
    let session = Arc::new(McpSession::new(
        rt.url,
        token,
        #[cfg(not(feature = "legacy-spec"))]
        rt.peer_mode.clone(),
    ));

    #[cfg(feature = "client-oauth")]
    let auth = match rt.oauth {
        Some(oauth) => ClientAuth::OAuth(oauth),
        None => ClientAuth::from_static(rt.access_token),
    };
    #[cfg(not(feature = "client-oauth"))]
    let auth = ClientAuth::from_static(rt.access_token);

    // The SSE task arms itself only when a legacy `initialize` handshake
    // completes (`session.initialized()` fires exclusively for the
    // `initialize` method) -- against a 2026-07-28 peer it stays parked until
    // cancellation, so the stateless 2026-07-28 transport still issues only POSTs.
    tokio::join!(
        handle_connection(
            session.clone(),
            rt.rx,
            rt.tx.clone(),
            auth.clone(),
            #[cfg(not(feature = "legacy-spec"))]
            rt.param_headers.clone(),
            #[cfg(feature = "client-tls")]
            rt.tls_config.clone()
        ),
        start_sse_connection(
            session.clone(),
            rt.tx.clone(),
            auth.clone(),
            #[cfg(feature = "client-tls")]
            rt.tls_config.clone()
        )
    );
}

async fn handle_connection(
    session: Arc<McpSession>,
    mut sender_rx: mpsc::Receiver<Message>,
    recv_tx: mpsc::Sender<Result<Message, Error>>,
    auth: ClientAuth,
    #[cfg(not(feature = "legacy-spec"))] param_registry: crate::shared::param_headers::Registry,
    #[cfg(feature = "client-tls")] tls_config: Option<ClientTlsConfig>,
) {
    #[cfg(not(feature = "client-tls"))]
    let client = match create_client() {
        Ok(client) => client,
        Err(_err) => {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "HTTP client error: {_err:#}");
            return;
        }
    };

    #[cfg(feature = "client-tls")]
    let client = match create_client(tls_config) {
        Ok(client) => client,
        Err(_err) => {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "HTTP client error: {_err:#}");
            return;
        }
    };

    let token = session.cancellation_token();
    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => return,
            req = sender_rx.recv() => {
                let Some(req) = req else {
                    #[cfg(feature = "tracing")]
                    tracing::error!(logger = "neva", "Unexpected messaging error");
                    break;
                };
                // A cancel naming a request whose reply is a long-lived stream
                // ends it the way this transport can: by closing the body. The
                // notification still goes out -- a peer may want the reason --
                // but the close is what the server acts on.
                #[cfg(not(feature = "legacy-spec"))]
                abort_cancelled_stream(&req, &session);

                // Tracked here rather than inside the spawned task, so that a
                // cancel arriving right behind a listen -- which is exactly
                // what a dropped `Client::listen` sends -- finds the handle.
                // Registering it in the task would leave the ordering to the
                // scheduler; registering it in this loop makes it the order the
                // messages arrived in.
                #[cfg(not(feature = "legacy-spec"))]
                let abort = track_listen(&req, &session);

                crate::spawn_fair!(send_request(
                    client.clone(),
                    session.clone(),
                    req,
                    recv_tx.clone(),
                    auth.clone(),
                    #[cfg(not(feature = "legacy-spec"))]
                    param_registry.clone(),
                    #[cfg(not(feature = "legacy-spec"))]
                    abort,
                ));
            }
        }
    }
}

/// Classifies a non-JSON-RPC HTTP reply into the code and message the
/// originating requests are completed with, carrying the HTTP status so
/// the caller can tell *why* the body wasn't JSON-RPC.
///
/// The code matters beyond diagnostics: `ParseError` is one of the
/// dual-mode fallback triggers (issue #84), so it must be produced *only*
/// for replies that genuinely suggest "this peer doesn't know the 2026-07-28
/// method/route" -- an allowlist, not a catch-all:
///
/// * any `2xx` -- a legacy peer answering `server/discover` on the wire but
///   with a body neva can't read as JSON-RPC, most notably an error code
///   outside its `ErrorCode` set (the TS SDK's `-32000` "server not
///   initialized" family);
/// * `400` / `404` / `405` / `406` -- routers and legacy servers rejecting
///   the unknown method or endpoint outright.
///
/// Everything else is an upstream failure that says nothing about the
/// peer's protocol generation and must surface as-is
/// (`InternalError`, like "Connection closed"):
/// `401`/`403`/`407` (authentication -- otherwise a failed login against a
/// valid 2026-07-28 server reads as "legacy"), `429` (rate limit) and every `5xx`
/// (reverse-proxy outage, gateway timeout). Treating those as legacy
/// evidence would silently drop the 2026-07-28 headers, retry `initialize` into
/// the same outage, and bury the real cause.
#[inline]
fn parse_failure(status: reqwest::StatusCode, err: &impl std::fmt::Display) -> (ErrorCode, String) {
    let unsupported_route = matches!(status.as_u16(), 400 | 404 | 405 | 406);
    let code = if status.is_success() || unsupported_route {
        ErrorCode::ParseError
    } else {
        ErrorCode::InternalError
    };
    (code, format!("HTTP {status}: {err}"))
}

/// Whether a reply is SSE-framed rather than a single JSON document.
fn is_event_stream(headers: &reqwest::header::HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|ct| ct.split(';').next())
        .is_some_and(|essence| essence.trim().eq_ignore_ascii_case("text/event-stream"))
}

/// The ids of the requests `msg` carries (empty for notifications and
/// notification-only batches).
fn request_ids(msg: &Message) -> Vec<crate::types::RequestId> {
    match msg {
        Message::Request(r) => vec![r.id()],
        Message::Batch(batch) => batch
            .iter()
            .filter_map(|envelope| match envelope {
                crate::types::MessageEnvelope::Request(r) => Some(r.id()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[inline]
#[cfg(not(feature = "client-tls"))]
fn create_client() -> Result<reqwest::Client, Error> {
    reqwest::Client::builder().build().map_err(Error::from)
}

#[inline]
#[cfg(feature = "client-tls")]
fn create_client(mut tls_config: Option<ClientTlsConfig>) -> Result<reqwest::Client, Error> {
    let mut builder = reqwest::ClientBuilder::new();
    if let Some(ca_cert) = tls_config.as_mut().and_then(|tls| tls.ca.take()) {
        builder = builder.add_root_certificate(ca_cert);
    }
    if let Some(identity) = tls_config.as_mut().and_then(|tls| tls.identity.take()) {
        builder = builder.identity(identity);
    }
    if tls_config.is_some_and(|tls| !tls.certs_verification) {
        builder = builder.danger_accept_invalid_certs(true);
    }
    builder.build().map_err(Error::from)
}

impl From<reqwest::Error> for Error {
    #[inline]
    fn from(err: reqwest::Error) -> Self {
        Error::new(ErrorCode::ParseError, err.to_string())
    }
}

// These tests exercise the SSE GET stream path, which serves legacy
// peers -- compiled under both flags for the dual-mode client.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::http::ServiceUrl;

    /// `insufficient_scope` is a value of the challenge's `error` parameter,
    /// and the same bytes turn up where they mean the opposite: describing the
    /// code in prose, or inside a scope name. Mistaking those for the error
    /// sends the caller through an interactive flow -- discarding a valid token
    /// -- to retry a request re-authorization cannot fix.
    #[test]
    #[cfg(feature = "client-oauth")]
    fn only_the_challenge_error_parameter_says_the_scope_is_short() {
        let headers_of = |value: &str| {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                reqwest::header::WWW_AUTHENTICATE,
                value.parse().expect("a header value"),
            );
            headers
        };

        let challenged = |value: &str| insufficient_scope(&headers_of(value));

        assert!(challenged(r#"Bearer error="insufficient_scope""#));
        assert!(challenged(
            r#"Bearer realm="mcp", error="insufficient_scope", scope="admin""#
        ));

        // The words, but as prose about a different error.
        assert!(!challenged(
            r#"Bearer error="invalid_token", error_description="missing insufficient_scope claim""#
        ));
        // The words, but as part of a scope name.
        assert!(!challenged(
            r#"Bearer error="invalid_token", scope="insufficient_scope_admin""#
        ));
        // No error parameter at all: a resource server refusing the caller.
        assert!(!challenged(r#"Bearer realm="mcp""#));
        assert!(!challenged("Basic realm=\"mcp\""));

        // And no challenge at all.
        assert!(!insufficient_scope(&reqwest::header::HeaderMap::new()));

        // `WWW-Authenticate` may be sent more than once, and the Bearer
        // challenge need not come first. Reading only the first value would
        // answer as if none had been offered.
        let mut headers = reqwest::header::HeaderMap::new();
        headers.append(
            reqwest::header::WWW_AUTHENTICATE,
            r#"Basic realm="legacy""#.parse().expect("a header value"),
        );
        headers.append(
            reqwest::header::WWW_AUTHENTICATE,
            r#"Bearer error="insufficient_scope", scope="admin""#
                .parse()
                .expect("a header value"),
        );
        assert!(
            insufficient_scope(&headers),
            "the Bearer challenge counts wherever in the list it sits"
        );

        // Several challenges inside *one* value are the parser's job, and it
        // does them -- so this must not regress into scanning only the first.
        assert!(challenged(
            r#"Basic realm="legacy", Bearer error="insufficient_scope""#
        ));

        // Two *Bearer* challenges, the applicable one second. Stopping at the
        // first that parses answers "not a step-up" on a response that asked
        // for one, and the token gets refreshed into the same refusal.
        let mut headers = reqwest::header::HeaderMap::new();
        headers.append(
            reqwest::header::WWW_AUTHENTICATE,
            r#"Bearer realm="legacy", error="invalid_token""#
                .parse()
                .expect("a header value"),
        );
        headers.append(
            reqwest::header::WWW_AUTHENTICATE,
            r#"Bearer realm="mcp", error="insufficient_scope", scope="admin""#
                .parse()
                .expect("a header value"),
        );
        assert!(
            insufficient_scope(&headers),
            "the challenge that names the code is the one that answers"
        );
        // And it is the one handed to the flow, or the step-up would go asking
        // for the grant it already had: the scope it was short of lives on that
        // challenge and nowhere else.
        assert_eq!(
            bearer_challenge(&headers).as_deref(),
            Some(r#"Bearer realm="mcp", error="insufficient_scope", scope="admin""#),
            "the flow must be given the challenge that says what is missing"
        );

        // With no such challenge, the first that parses is as good as any --
        // and a Bearer behind a Basic is still found.
        let mut plain = reqwest::header::HeaderMap::new();
        plain.append(
            reqwest::header::WWW_AUTHENTICATE,
            r#"Basic realm="legacy""#.parse().expect("a header value"),
        );
        plain.append(
            reqwest::header::WWW_AUTHENTICATE,
            r#"Bearer resource_metadata="https://rs.example/.well-known/oauth-protected-resource""#
                .parse()
                .expect("a header value"),
        );
        assert_eq!(
            bearer_challenge(&plain).as_deref(),
            Some(
                r#"Bearer resource_metadata="https://rs.example/.well-known/oauth-protected-resource""#
            )
        );
        assert!(!insufficient_scope(&plain));

        // The same list, in *one* value. RFC 9110 lets a server send it that
        // way, and the dependency's parser stops at the second scheme -- so
        // walking header values alone would never reach the challenge that
        // matters.
        assert!(challenged(
            r#"Bearer realm="legacy", error="invalid_token", Bearer realm="mcp", error="insufficient_scope", scope="admin""#
        ));
        let mut combined = reqwest::header::HeaderMap::new();
        combined.insert(
            reqwest::header::WWW_AUTHENTICATE,
            r#"Bearer realm="legacy", error="invalid_token", Bearer realm="mcp", error="insufficient_scope", scope="admin""#
                .parse()
                .expect("a header value"),
        );
        assert_eq!(
            bearer_challenge(&combined).as_deref(),
            Some(r#"Bearer realm="mcp", error="insufficient_scope", scope="admin""#),
            "the applicable challenge is handed over on its own"
        );

        // The demanded scope has to survive the split, or the step-up asks for
        // nothing and the retry is refused identically. Spaced around the `=`,
        // which RFC 9110 allows and the challenge parser accepts.
        let spaced = r#"Bearer error = "insufficient_scope", scope = "admin""#;
        assert!(challenged(spaced));
        let parsed = volga_oauth_client::BearerChallenge::parse(
            bearer_challenge(&headers_of(spaced))
                .as_deref()
                .expect("a challenge"),
        )
        .expect("it parses");
        assert_eq!(parsed.scope(), Some("admin"));
    }

    /// A comma inside a quoted string separates nothing, and a parameter is not
    /// a scheme however much it looks like one at a glance.
    #[test]
    #[cfg(feature = "client-oauth")]
    fn a_header_value_is_split_on_challenge_boundaries() {
        assert_eq!(
            bearer_challenges(r#"Bearer scope="a,b", error="insufficient_scope""#),
            vec![r#"Bearer scope="a,b", error="insufficient_scope""#],
            "a quoted comma is part of the value, not a list separator"
        );
        assert_eq!(
            bearer_challenges(r#"Basic realm="legacy", Bearer realm="mcp""#),
            vec![r#"Bearer realm="mcp""#],
            "the other scheme's parameters stay with it"
        );
        assert_eq!(
            bearer_challenges(r#"Bearer, Bearer error="insufficient_scope""#),
            vec![r#"Bearer"#, r#"Bearer error="insufficient_scope""#],
            "a bare challenge is still a challenge"
        );
        assert!(bearer_challenges(r#"Basic realm="legacy""#).is_empty());
        // `token BWS "=" BWS value` is a parameter, not a scheme -- splitting
        // there would leave the challenge without the scope it demanded.
        assert_eq!(
            bearer_challenges(r#"Bearer error="insufficient_scope", scope = "admin""#),
            vec![r#"Bearer error="insufficient_scope", scope = "admin""#]
        );
        // A token68 payload has whitespace after its scheme and an `=` of its
        // own, and is still a challenge of another scheme.
        assert!(bearer_challenges("Basic dXNlcjpwYXNz==").is_empty());
        // A quoted escape must not end the string early and turn the rest of
        // the value into challenges of its own.
        assert_eq!(
            bearer_challenges(r#"Bearer error_description="say \" then, stop""#),
            vec![r#"Bearer error_description="say \" then, stop""#]
        );
    }

    /// A `DPoP` challenge is one this client answers, and its parameters are
    /// read the same way a `Bearer` one's are (RFC 9449 section 7.1). A scheme
    /// neither of those is still left alone.
    #[cfg(feature = "client-oauth-dpop")]
    #[test]
    fn a_dpop_challenge_is_one_this_client_answers() {
        let value = r#"DPoP error="use_dpop_nonce", resource_metadata="https://mcp.example/prm""#;
        assert_eq!(bearer_challenges(value), vec![value]);
        assert!(bearer_challenges(r#"Negotiate realm="ad""#).is_empty());

        let parsed = oauth::parse_challenge(value).expect("it parses");
        assert_eq!(parsed.scheme(), "DPoP");
        assert_eq!(parsed.resource_metadata(), Some("https://mcp.example/prm"));

        // Both schemes in one value: the DPoP one is not lost behind the
        // bearer one, and neither swallows the other's parameters.
        assert_eq!(
            bearer_challenges(r#"Bearer realm="mcp", DPoP error="use_dpop_nonce""#),
            vec![r#"Bearer realm="mcp""#, r#"DPoP error="use_dpop_nonce""#]
        );
    }

    /// A nonce is not a demand to repeat the request -- the `error` is.
    #[cfg(feature = "client-oauth-dpop")]
    #[test]
    fn only_a_use_dpop_nonce_refusal_asks_for_a_retry() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("dpop-nonce", "n-1".parse().unwrap());
        assert!(
            !use_dpop_nonce(&headers),
            "a nonce handed out alongside an answer is for the next request"
        );

        headers.insert(
            reqwest::header::WWW_AUTHENTICATE,
            r#"DPoP error="use_dpop_nonce""#.parse().unwrap(),
        );
        assert!(use_dpop_nonce(&headers));

        headers.insert(
            reqwest::header::WWW_AUTHENTICATE,
            r#"DPoP error="invalid_token""#.parse().unwrap(),
        );
        assert!(
            !use_dpop_nonce(&headers),
            "an expired token is not answered by repeating the request"
        );
    }

    /// Every request gets its own proof, bound to that request's method, URL
    /// and token (RFC 9449 sections 4.2 and 4.3). Reusing one is a replay, and
    /// a resource that remembers `jti` refuses it.
    #[cfg(feature = "client-oauth-dpop")]
    #[test]
    fn a_dpop_credential_signs_a_fresh_proof_per_request() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};

        let key = oauth::Dpop::generate().expect("a key");
        let credential = Credential::Dpop {
            tokens: Arc::new(oauth::TokenSet {
                access_token: "bound-token".into(),
                token_type: "DPoP".into(),
                refresh_token: None,
                scope: None,
                id_token: None,
                expires_at: None,
                dpop_jkt: Some(key.thumbprint().to_owned()),
            }),
            key,
        };

        let client = create_client(
            #[cfg(feature = "client-tls")]
            None,
        )
        .expect("a client");
        let url = "https://mcp.example.com/mcp";

        let claims = |method: &reqwest::Method| {
            let (built, nonce) = credential
                .attach(client.post(url), method, url, None)
                .expect("the proof signs");
            assert_eq!(nonce, None, "no server has handed one out yet");

            let built = built.build().expect("a request");
            assert_eq!(
                built.headers()[reqwest::header::AUTHORIZATION],
                "DPoP bound-token",
                "the token is presented under the DPoP scheme, not Bearer"
            );

            let proof = built.headers()["dpop"].to_str().unwrap().to_owned();
            let claims = B64.decode(proof.split('.').nth(1).unwrap()).unwrap();
            serde_json::from_slice::<serde_json::Value>(&claims).unwrap()
        };

        let first = claims(&reqwest::Method::POST);
        let second = claims(&reqwest::Method::POST);

        assert_eq!(first["htm"], "POST");
        assert_eq!(first["htu"], url);
        // base64url(sha256("bound-token")), the `ath` of RFC 9449 section 4.2
        assert_eq!(
            first["ath"], "1UZzyLKzndbtZN8OcOiPRvW7SJ-rr1VMAPv_rM0MfgE",
            "the proof binds to the token the request presents"
        );
        assert_ne!(first["jti"], second["jti"], "a proof is not reused");

        assert_eq!(claims(&reqwest::Method::GET)["htm"], "GET");
    }

    fn make_session() -> Arc<McpSession> {
        Arc::new(McpSession::new(
            ServiceUrl::default(),
            CancellationToken::new(),
            #[cfg(not(feature = "legacy-spec"))]
            Default::default(),
        ))
    }

    /// An exchange that builds its `POST` twice must send the same
    /// `Mcp-Param-*` headers both times.
    ///
    /// The second build is the managed-OAuth retry, and the headers may be
    /// mirrored on a grace: one call's worth, granted because the server refused
    /// the first attempt for missing them. Reading the registry again there
    /// finds the grace spent and the listing stale, so the retry would go out
    /// bare -- and be refused for exactly what the recovery had just fixed.
    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn a_retried_post_mirrors_what_the_first_one_did() {
        use crate::shared::param_headers::{ParamHeader, Registration};

        let session = make_session();
        let registry: crate::shared::param_headers::Registry = Default::default();
        // `ttlMs: 0` -- stale on arrival, which is what an absent `ttlMs` means
        // too, so the grace is the only thing that lets this call mirror at all.
        registry.insert(
            "route".to_string(),
            Registration::new(
                vec![ParamHeader {
                    path: vec!["region".into()],
                    header: "Region".into(),
                }],
                0,
                true,
            ),
        );

        let req = Message::Request(crate::types::Request::new(
            Some(crate::types::RequestId::Number(1)),
            crate::types::tool::commands::CALL,
            Some(serde_json::json!({
                "name": "route",
                "arguments": { "region": "us-west1" }
            })),
        ));

        let mirrored = mirrored_param_headers(&session, &req, &registry);
        assert_eq!(
            mirrored,
            vec![("Mcp-Param-Region".to_string(), "us-west1".to_string())],
            "the grace covers this call"
        );
        assert!(
            mirrored_param_headers(&session, &req, &registry).is_empty(),
            "and reading is what spends it -- hence reading once"
        );

        let client = create_client(
            #[cfg(feature = "client-tls")]
            None,
        )
        .expect("a client");
        for attempt in ["first", "retry"] {
            let built = build_post(&client, &session, &req, &mirrored)
                .build()
                .expect("a request");
            assert_eq!(
                built
                    .headers()
                    .get("Mcp-Param-Region")
                    .and_then(|v| v.to_str().ok()),
                Some("us-west1"),
                "the {attempt} attempt must carry the mirrored header"
            );
        }
    }

    /// A resumption refused for a credential must not cost the answer.
    ///
    /// The wait before reconnecting is the server's to name, and it can outlast
    /// the token the original `POST` went out with. The `POST` and the
    /// standalone `GET` both re-authorize once on a `401`; this path treating it
    /// as final loses the terminal response the reconnection went back for, and
    /// the request fails with an `InternalError` over a credential the client
    /// could simply have renewed.
    #[cfg(feature = "client-oauth")]
    #[tokio::test]
    async fn a_resumption_refused_for_its_token_authorizes_and_tries_again() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // One socket playing both parts: the MCP endpoint that refuses the
        // resumption until it carries the granted token, and the authorization
        // server that issues it.
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 8192];
                let read = stream.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..read]).to_string();
                let root = format!("http://{addr}");

                let resp = if request.starts_with("GET /mcp") {
                    if request.contains("Bearer granted-token") {
                        let body = "id: 2\nevent: message\ndata: \
                             {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n";
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                    } else {
                        format!(
                            "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer resource_metadata=\"{root}/.well-known/oauth-protected-resource/mcp\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                    }
                } else {
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
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                };
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        });

        let url = format!("http://{addr}/mcp");
        // `From<&str>` takes `addr[/endpoint]`, without a scheme; the endpoint
        // defaults to `/mcp`, which is where this mock listens.
        let session = Arc::new(McpSession::new(
            ServiceUrl::from(addr.to_string().as_str()),
            CancellationToken::new(),
            #[cfg(not(feature = "legacy-spec"))]
            Default::default(),
        ));
        let config = oauth::OAuthClientConfig::default()
            .require_https(false)
            .with_handler(EchoesState);
        let auth = ClientAuth::OAuth(Arc::new(
            oauth::OAuthSession::new(config, &url).expect("a session"),
        ));

        let (tx, mut rx) = mpsc::channel(2);
        let owed = resume_stream(
            &create_client(
                #[cfg(feature = "client-tls")]
                None,
            )
            .expect("a client"),
            &session,
            &auth,
            "1",
            Some(0),
            &tx,
            &[crate::types::RequestId::Number(1)],
        )
        .await;

        assert!(
            owed.is_empty(),
            "the resumption must recover the answer rather than give up on a 401"
        );
        assert!(matches!(rx.try_recv(), Ok(Ok(Message::Response(_)))));
    }

    /// The claims of the DPoP proof on a raw request, or `None` when it
    /// carries none.
    #[cfg(feature = "client-oauth-dpop")]
    fn proof_claims(request: &str) -> Option<serde_json::Value> {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};

        let proof = request.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("dpop")
                .then(|| value.trim())
        })?;

        let claims = B64.decode(proof.split('.').nth(1)?).ok()?;
        serde_json::from_slice(&claims).ok()
    }

    /// The whole DPoP exchange, end to end, in the shape the
    /// `auth/dpop-nonce` conformance scenario drives it: a resource that
    /// challenges with the `DPoP` scheme, an authorization server that demands
    /// a nonce before it will issue, and a resource that demands one of its
    /// own before it will serve.
    ///
    /// Each of those is a place a bearer-only client stops. The challenge is
    /// in a scheme it does not read, so it never authorizes; the token request
    /// carries no proof, so it gets no bound token; and the resource's nonce
    /// round looks like a credential failure, which spends the one
    /// re-authorization a `401` is allowed and arrives back at the same
    /// refusal.
    #[cfg(feature = "client-oauth-dpop")]
    #[tokio::test]
    async fn a_dpop_session_answers_both_nonce_challenges_and_is_served() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        const AS_NONCE: &str = "as-nonce-1";
        const RS_NONCE: &str = "rs-nonce-1";

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // One socket playing both parts, as the other transport tests do.
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 8192];
                let read = stream.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..read]).to_string();
                let root = format!("http://{addr}");
                let claims = proof_claims(&request);
                let carried = |nonce: &str| {
                    claims.as_ref().and_then(|claims| claims["nonce"].as_str()) == Some(nonce)
                };

                let resp = if request.starts_with("POST /mcp") {
                    if !request.contains("authorization: DPoP bound-token") {
                        format!(
                            "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: DPoP resource_metadata=\"{root}/.well-known/oauth-protected-resource/mcp\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                    } else if !carried(RS_NONCE) {
                        // RFC 9449 section 9: the resource hands out its nonce
                        // by refusing once, and the refusal is not about the
                        // token.
                        let body = r#"{"error":"use_dpop_nonce"}"#;
                        format!(
                            "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: DPoP error=\"use_dpop_nonce\"\r\nDPoP-Nonce: {RS_NONCE}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                    } else {
                        let body = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                    }
                } else if request.starts_with("POST /token") && !carried(AS_NONCE) {
                    // RFC 9449 section 8, the same round one step earlier.
                    let body = r#"{"error":"use_dpop_nonce"}"#;
                    format!(
                        "HTTP/1.1 400 Bad Request\r\nDPoP-Nonce: {AS_NONCE}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                } else {
                    let body = if request.contains("/.well-known/oauth-protected-resource") {
                        format!(r#"{{"resource":"{root}/mcp","authorization_servers":["{root}"]}}"#)
                    } else if request.contains("/.well-known/") {
                        format!(
                            r#"{{"issuer":"{root}","token_endpoint":"{root}/token",
                                 "authorization_endpoint":"{root}/authorize",
                                 "registration_endpoint":"{root}/register",
                                 "dpop_signing_alg_values_supported":["ES256"],
                                 "response_types_supported":["code"]}}"#
                        )
                    } else if request.contains("/register") {
                        r#"{"client_id":"registered-client"}"#.to_string()
                    } else {
                        r#"{"access_token":"bound-token","token_type":"DPoP","expires_in":3600}"#
                            .to_string()
                    };
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                };
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        });

        let url = format!("http://{addr}/mcp");
        let session = Arc::new(McpSession::new(
            ServiceUrl::from(addr.to_string().as_str()),
            CancellationToken::new(),
            #[cfg(not(feature = "legacy-spec"))]
            Default::default(),
        ));
        let config = oauth::OAuthClientConfig::default()
            .require_https(false)
            .with_dpop_auto()
            .with_handler(EchoesState);
        let oauth_session = Arc::new(oauth::OAuthSession::new(config, &url).expect("a session"));
        let auth = ClientAuth::OAuth(oauth_session.clone());

        let (tx, mut rx) = mpsc::channel(2);
        exchange(
            create_client(
                #[cfg(feature = "client-tls")]
                None,
            )
            .expect("a client"),
            session,
            Message::Request(crate::types::Request::new(
                Some(crate::types::RequestId::Number(1)),
                "ping",
                None::<serde_json::Value>,
            )),
            tx,
            auth,
            #[cfg(not(feature = "legacy-spec"))]
            Default::default(),
        )
        .await;

        assert!(
            matches!(rx.try_recv(), Ok(Ok(Message::Response(_)))),
            "the call must be served once both nonce rounds are answered"
        );

        let credential = oauth_session.credential().expect("a credential");
        assert!(
            matches!(credential, Credential::Dpop { .. }),
            "the session must hold the bound token, not a bearer one"
        );
        assert_eq!(credential.access_token(), "bound-token");
    }

    /// Completes the flow without a browser by reading the `state` back off the
    /// authorization URL -- which is what the redirect would have carried.
    #[cfg(feature = "client-oauth")]
    struct EchoesState;

    #[cfg(feature = "client-oauth")]
    impl crate::auth::oauth::AuthorizationHandler for EchoesState {
        fn redirect_uri(
            &self,
        ) -> futures_util::future::BoxFuture<'_, Result<String, crate::error::Error>> {
            Box::pin(async { Ok("http://127.0.0.1:8919/callback".to_string()) })
        }

        fn authorize(
            &self,
            url: String,
        ) -> futures_util::future::BoxFuture<
            '_,
            Result<crate::auth::oauth::CallbackParams, crate::error::Error>,
        > {
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
                Ok(crate::auth::oauth::CallbackParams {
                    code: "the-code".into(),
                    state,
                    iss: None,
                })
            })
        }
    }

    // A minimal valid JSON-RPC notification that Message will accept
    const VALID_MSG: &str = r#"{"jsonrpc":"2.0","method":"ping"}"#;

    /// Only statuses that actually suggest an unknown method/route may
    /// yield `ParseError` -- the dual-mode fallback trigger. Upstream
    /// failures (auth, rate limit, 5xx) must stay `InternalError` so a
    /// valid 2026-07-28 peer is never mistaken for a legacy one.
    #[test]
    fn parse_failure_classifies_statuses() {
        let cases = [
            // legacy evidence: the peer answered, the body just isn't JSON-RPC
            (200, ErrorCode::ParseError),
            (202, ErrorCode::ParseError),
            (400, ErrorCode::ParseError),
            (404, ErrorCode::ParseError),
            (405, ErrorCode::ParseError),
            (406, ErrorCode::ParseError),
            // upstream failures: say nothing about the protocol generation
            (401, ErrorCode::InternalError),
            (403, ErrorCode::InternalError),
            (407, ErrorCode::InternalError),
            (429, ErrorCode::InternalError),
            (500, ErrorCode::InternalError),
            (502, ErrorCode::InternalError),
            (503, ErrorCode::InternalError),
            (504, ErrorCode::InternalError),
        ];

        for (status, expected) in cases {
            let status = reqwest::StatusCode::from_u16(status).unwrap();
            let (code, reason) = parse_failure(status, &"boom");
            assert_eq!(code, expected, "wrong code for HTTP {status}");
            assert!(
                reason.contains(status.as_str()),
                "the status must be carried in the message, got: {reason}"
            );
        }
    }

    #[tokio::test]
    async fn it_advances_last_event_id_on_successful_delivery() {
        let session = make_session();
        let (tx, mut rx) = mpsc::channel(1);

        let event = sse_stream::Sse::default().id("evt-1").data(VALID_MSG);
        handle_event(event, &session, &tx).await;

        assert_eq!(session.last_event_id(), Some("evt-1".to_string()));
        assert!(rx.try_recv().is_ok(), "message should have been delivered");
    }

    #[tokio::test]
    async fn it_does_not_advance_last_event_id_on_parse_failure() {
        let session = make_session();
        let (tx, _rx) = mpsc::channel(1);

        let event = sse_stream::Sse::default()
            .id("evt-bad")
            .data("not { valid json");
        handle_event(event, &session, &tx).await;

        assert!(session.last_event_id().is_none());
    }

    #[tokio::test]
    async fn it_does_not_advance_last_event_id_when_channel_closed() {
        let session = make_session();
        let (tx, rx) = mpsc::channel(1);
        drop(rx);

        let event = sse_stream::Sse::default().id("evt-dropped").data(VALID_MSG);
        handle_event(event, &session, &tx).await;

        assert!(session.last_event_id().is_none());
    }

    #[tokio::test]
    async fn it_advances_last_event_id_for_non_message_event() {
        let session = make_session();
        let (tx, _rx) = mpsc::channel(1);

        // Non-message SSE event (has event: field) -- no data sent to channel, but
        // ID should still advance so the server does not replay it on reconnect.
        let event = sse_stream::Sse::default()
            .id("evt-keepalive")
            .event("keepalive");
        handle_event(event, &session, &tx).await;

        assert_eq!(session.last_event_id(), Some("evt-keepalive".to_string()));
    }

    #[tokio::test]
    async fn it_does_not_advance_last_event_id_when_data_is_absent() {
        let session = make_session();
        let (tx, _rx) = mpsc::channel(1);

        // A message frame (no event: field) but data is None
        let event = sse_stream::Sse::default().id("evt-empty");
        handle_event(event, &session, &tx).await;

        assert!(session.last_event_id().is_none());
    }

    /// `message` is the default SSE event type, so naming it explicitly must be
    /// treated exactly like omitting it.
    #[test]
    fn explicitly_named_message_events_count_as_messages() {
        let cases = [
            (None, true),
            (Some("message"), true),
            (Some(" message "), true),
            (Some("Message"), false),
            (Some("keepalive"), false),
            (Some("endpoint"), false),
        ];

        for (kind, expected) in cases {
            let event = match kind {
                Some(kind) => sse_stream::Sse::default().event(kind),
                None => sse_stream::Sse::default(),
            };
            assert_eq!(
                is_message_event(&event),
                expected,
                "wrong verdict for event type {kind:?}"
            );
        }
    }

    /// A peer that frames its stream with `event: message` must have its payload
    /// delivered on both SSE paths -- the standalone `GET` and the POST reply.
    #[tokio::test]
    async fn named_message_events_are_delivered_on_both_sse_paths() {
        let response = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;

        // Standalone GET stream: delivered, and the last event id advances.
        let session = make_session();
        let (tx, mut rx) = mpsc::channel(1);
        let event = sse_stream::Sse::default()
            .id("evt-named")
            .event("message")
            .data(response);
        handle_event(event, &session, &tx).await;
        assert!(rx.try_recv().is_ok(), "GET frame must be delivered");
        assert_eq!(session.last_event_id(), Some("evt-named".to_string()));

        // Request-scoped POST reply, driven through the real drain loop: the
        // notification and the response are both delivered, and the stream counts
        // as answered so the request is not failed as truncated.
        let (tx, mut rx) = mpsc::channel(4);
        let frames = vec![
            Ok(sse_stream::Sse::default()
                .event("message")
                .data(r#"{"jsonrpc":"2.0","method":"notifications/message"}"#)),
            Ok(sse_stream::Sse::default().event("message").data(response)),
        ];
        assert!(
            drain_post_sse(
                futures_util::stream::iter(frames),
                &tx,
                &[crate::types::RequestId::Number(1)],
            )
            .await
            .owed
            .is_empty(),
            "the POST stream must leave nothing owed"
        );
        assert!(matches!(rx.try_recv(), Ok(Ok(Message::Notification(_)))));
        assert!(matches!(rx.try_recv(), Ok(Ok(Message::Response(_)))));
    }

    /// A priming frame carries no message, and is exactly where a server states
    /// where to resume from and how long to wait -- so both have to be taken off
    /// a frame the message path skips.
    ///
    /// Both belong to this stream alone -- a legacy session also runs the
    /// standalone `GET`, with its own position and its own reconnection time,
    /// and either one shared would have each stream reconnect on the other's
    /// terms: from a place it never reached, or after a delay named for
    /// somebody else.
    #[tokio::test]
    async fn a_priming_frame_still_states_where_to_resume_from() {
        let session = make_session();
        session.set_last_event_id("get-stream-7".to_string());
        session.set_retry(9_000);
        let (tx, mut rx) = mpsc::channel(2);

        let mut priming = sse_stream::Sse::default().id("event-1");
        priming.retry = Some(500);
        let frames = vec![Ok(priming)];

        let drained = drain_post_sse(
            futures_util::stream::iter(frames),
            &tx,
            &[crate::types::RequestId::Number(1)],
        )
        .await;

        assert_eq!(
            drained.owed,
            vec![crate::types::RequestId::Number(1)],
            "a priming frame answers nothing"
        );
        assert!(rx.try_recv().is_err(), "and delivers nothing");
        assert_eq!(
            drained.last_event_id,
            Some("event-1".to_string()),
            "this stream resumes from where this stream got to"
        );
        assert_eq!(
            drained.retry,
            Some(500),
            "and after the delay this stream was given"
        );
        assert_eq!(
            session.last_event_id(),
            Some("get-stream-7".to_string()),
            "leaving the standalone GET's own position alone"
        );
        assert_eq!(
            session.retry_delay(SSE_RECONNECT_DELAY),
            Duration::from_millis(9_000),
            "and its own reconnection delay with it"
        );
    }

    /// Frames of some other event type are skipped without answering the
    /// request, and a stream that carries only those ends unanswered.
    #[tokio::test]
    async fn drain_post_sse_skips_other_event_types() {
        let (tx, mut rx) = mpsc::channel(2);
        let frames = vec![
            Ok(sse_stream::Sse::default().event("keepalive").data("{}")),
            Ok(sse_stream::Sse::default()
                .event("endpoint")
                .data(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#)),
        ];
        assert_eq!(
            drain_post_sse(
                futures_util::stream::iter(frames),
                &tx,
                &[crate::types::RequestId::Number(1)],
            )
            .await
            .owed,
            vec![crate::types::RequestId::Number(1)]
        );
        assert!(rx.try_recv().is_err(), "no frame should be delivered");
    }

    /// A request-scoped SSE `POST` reply must be recognized as *answered* only
    /// once its terminal reply arrives -- that flag is what tells a truncated
    /// stream (which has to fail the pending request) from an orderly one.
    #[tokio::test]
    async fn forward_sse_message_flags_only_terminal_replies() {
        let cases = [
            // (frame, is_terminal)
            (
                r#"{"jsonrpc":"2.0","method":"notifications/message"}"#,
                false,
            ),
            (r#"{"jsonrpc":"2.0","id":1,"result":{}}"#, true),
            (r#"[{"jsonrpc":"2.0","id":1,"result":{}}]"#, true),
            // A batch is not terminal by virtue of being a batch: a
            // subscription stream may deliver its acknowledgment and its events
            // this way, and the response is still to come.
            (
                r#"[{"jsonrpc":"2.0","method":"notifications/subscriptions/acknowledged"},
                    {"jsonrpc":"2.0","method":"notifications/tools/list_changed"}]"#,
                false,
            ),
            // ...but one that carries a response among them is.
            (
                r#"[{"jsonrpc":"2.0","method":"notifications/message"},
                    {"jsonrpc":"2.0","id":1,"result":{}}]"#,
                true,
            ),
            // Nor is a response terminal by virtue of being a response: this
            // `POST` never sent id 9, so nothing here can resolve its slot.
            (r#"{"jsonrpc":"2.0","id":9,"result":{}}"#, false),
            (r#"[{"jsonrpc":"2.0","id":9,"result":{}}]"#, false),
        ];

        for (frame, terminal) in cases {
            let (tx, mut rx) = mpsc::channel(1);
            let event = sse_stream::Sse::default().data(frame);
            let mut owed = vec![crate::types::RequestId::Number(1)];
            forward_sse_message(event, &tx, &mut owed).await;
            assert_eq!(owed.is_empty(), terminal, "wrong terminal flag for {frame}");
            assert!(rx.try_recv().is_ok(), "{frame} should still be delivered");
        }
    }

    /// A batched `POST` is answered request by request: a frame that resolves
    /// one of them leaves the others owed, and the caller must resume for --
    /// and ultimately fail -- only those.
    #[tokio::test]
    async fn a_batch_is_struck_off_one_answer_at_a_time() {
        let ids = [
            crate::types::RequestId::Number(1),
            crate::types::RequestId::Number(2),
        ];
        let (tx, _rx) = mpsc::channel(2);
        let mut owed = ids.to_vec();

        forward_sse_message(
            sse_stream::Sse::default().data(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#),
            &tx,
            &mut owed,
        )
        .await;
        assert_eq!(
            owed,
            vec![crate::types::RequestId::Number(2)],
            "only the answered request may be struck off"
        );

        forward_sse_message(
            sse_stream::Sse::default().data(r#"{"jsonrpc":"2.0","id":2,"result":{}}"#),
            &tx,
            &mut owed,
        )
        .await;
        assert!(owed.is_empty(), "the batch is now fully answered");
    }

    /// The resumed stream is the session's own long-lived `GET`: it does not
    /// close once it has replayed what was missed. Draining it to EOF would
    /// park this task on the session stream for the life of the client, so the
    /// drain has to stop the moment nothing is owed.
    #[tokio::test]
    async fn draining_stops_once_nothing_is_owed() {
        let (tx, mut rx) = mpsc::channel(8);
        // The response, then traffic that keeps coming -- as a live session
        // stream does. `pending()` after them stands in for a stream that never
        // ends: reaching it at all would hang this test.
        let frames = futures_util::stream::iter(vec![
            Ok(sse_stream::Sse::default().data(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#)),
            Ok(sse_stream::Sse::default()
                .data(r#"{"jsonrpc":"2.0","method":"notifications/message"}"#)),
        ])
        .chain(futures_util::stream::pending());

        let owed = tokio::time::timeout(
            Duration::from_secs(1),
            drain_post_sse(Box::pin(frames), &tx, &[crate::types::RequestId::Number(1)]),
        )
        .await
        .expect("the drain must return instead of holding the session stream open");

        assert!(owed.owed.is_empty());
        assert!(matches!(rx.try_recv(), Ok(Ok(Message::Response(_)))));
        assert!(
            rx.try_recv().is_err(),
            "nothing past the answer belongs to this exchange"
        );
    }

    #[tokio::test]
    async fn forward_sse_message_reports_unparseable_frame_as_unanswered() {
        let (tx, mut rx) = mpsc::channel(1);
        let event = sse_stream::Sse::default().data("not json");
        let mut owed = vec![crate::types::RequestId::Number(1)];
        forward_sse_message(event, &tx, &mut owed).await;
        assert_eq!(owed, vec![crate::types::RequestId::Number(1)]);
        assert!(
            rx.try_recv().is_err(),
            "a malformed frame must not reach the receive loop"
        );
    }

    /// Media types are case-insensitive and may carry parameters: mistaking such
    /// a reply for JSON would fail the request on an SSE-framed body.
    #[test]
    fn event_stream_media_type_is_matched_case_insensitively() {
        let cases = [
            ("text/event-stream", true),
            ("Text/Event-Stream", true),
            ("TEXT/EVENT-STREAM; charset=utf-8", true),
            ("text/event-stream ;charset=utf-8", true),
            ("application/json", false),
            ("text/event-streaming", false),
            ("application/json, text/event-stream", false),
        ];

        for (value, expected) in cases {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(CONTENT_TYPE, value.parse().unwrap());
            assert_eq!(
                is_event_stream(&headers),
                expected,
                "wrong verdict for content-type {value:?}"
            );
        }

        assert!(
            !is_event_stream(&reqwest::header::HeaderMap::new()),
            "a reply without content-type is not an SSE stream"
        );
    }
}

#[cfg(test)]
#[cfg(not(feature = "legacy-spec"))]
mod routing_hints_tests {
    use super::{name_param, routing_hints};
    use crate::transport::http::encode_header_value;
    use crate::types::notification::Notification;
    use crate::types::{Message, Request, RequestId};
    use serde_json::json;

    #[test]
    fn request_yields_method_and_no_name() {
        let req = Request::new::<()>(Some(RequestId::Number(1)), "tools/list", None);
        let msg = Message::Request(req);
        let hints = routing_hints(&msg).unwrap();
        assert_eq!(hints.0, "tools/list");
        assert!(hints.1.is_none());
    }

    #[test]
    fn tools_call_yields_method_and_tool_name() {
        let req = Request::new(
            Some(RequestId::Number(1)),
            "tools/call",
            Some(json!({"name": "echo", "arguments": {}})),
        );
        let msg = Message::Request(req);
        let hints = routing_hints(&msg).unwrap();
        assert_eq!(hints.0, "tools/call");
        assert_eq!(hints.1.as_deref(), Some("echo"));
    }

    /// The spec requires `Mcp-Name` on `tools/call`, `resources/read` and
    /// `prompts/get`, sourced from `params.name` / `params.uri`.
    #[test]
    #[cfg(not(feature = "legacy-spec"))]
    fn name_header_is_sourced_per_method() {
        use crate::types::{Request, RequestId};
        use serde_json::json;

        let cases = [
            ("tools/call", json!({ "name": "echo" }), Some("echo")),
            (
                "prompts/get",
                json!({ "name": "greeting" }),
                Some("greeting"),
            ),
            (
                "resources/read",
                json!({ "uri": "file:///a.txt" }),
                Some("file:///a.txt"),
            ),
            ("tools/list", json!({}), None),
        ];

        for (method, params, expected) in cases {
            let req = Request::new(Some(RequestId::Number(1)), method, Some(params));
            assert_eq!(name_param(&req).as_deref(), expected, "method: {method}");
        }
    }

    #[test]
    #[cfg(not(feature = "legacy-spec"))]
    fn header_values_are_encoded_when_not_ascii_safe() {
        // Plain ASCII passes through...
        assert_eq!(encode_header_value("us-west1"), "us-west1");
        // ...anything else travels Base64 behind the sentinel.
        assert_eq!(encode_header_value("caf\u{e9}"), "=?base64?Y2Fmw6k=?=");
        assert_eq!(encode_header_value(" lead"), "=?base64?IGxlYWQ=?=");
        assert_eq!(encode_header_value("trail "), "=?base64?dHJhaWwg?=");
        assert_eq!(encode_header_value("a\nb"), "=?base64?YQpi?=");
        // Horizontal tab is in the safe set the spec states, so an interior one
        // is sent as it came -- RFC 9110's `field-content` admits HTAB between
        // field-vchars. Only at an edge does it need encoding, and then by the
        // leading/trailing whitespace rule.
        //
        // The conformance suite's own predicate encodes any byte below 0x20,
        // interior tab included, but its `x-mcp-header` scenario only ever
        // sends a *leading* tab -- which both readings encode. Nothing on the
        // wire distinguishes them; this assertion is what keeps the library on
        // the spec's side of a difference no scenario exercises.
        assert_eq!(encode_header_value("a\tb"), "a\tb");
        assert_eq!(encode_header_value("\tindented"), "=?base64?CWluZGVudGVk?=");
        assert_eq!(encode_header_value("trailing\t"), "=?base64?dHJhaWxpbmcJ?=");
        // A plain value that looks like the sentinel must be encoded too, or a
        // server would decode something the client never encoded.
        assert_eq!(
            encode_header_value("=?base64?zzz?="),
            "=?base64?PT9iYXNlNjQ/enp6Pz0=?="
        );
    }

    #[test]
    fn notification_yields_method_only() {
        let n = Notification::new("notifications/cancelled", None);
        let msg = Message::Notification(n);
        let hints = routing_hints(&msg).unwrap();
        assert_eq!(hints.0, "notifications/cancelled");
        assert!(hints.1.is_none());
    }
}

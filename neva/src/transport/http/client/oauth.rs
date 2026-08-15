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

use url::{Host, ParseError, Url, form_urlencoded};

use volga_oauth_client::{
    AuthorizationServerMetadata, BearerChallenge, ClientConfig, ClientError, DiscoveryClient,
    OAuthClient, RegistrationClient, canonicalize_resource_uri, protected_resource_metadata_url,
};
pub use volga_oauth_client::{ClientMetadata, InMemoryTokenStore, TokenSet, TokenStore};

/// Client name sent with dynamic client registration when none is
/// configured.
const DEFAULT_CLIENT_NAME: &str = "neva MCP client";

mod config;
mod handler;
mod session;

pub use config::OAuthClientConfig;
pub use handler::{AuthorizationHandler, CallbackParams, LoopbackHandler};
#[cfg(test)]
use session::FlowState;
pub(crate) use session::OAuthSession;

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

    // Parsed once, and the three checks below read components off the result.
    // The port range is the one defect a parser can still find in a string the
    // canonicalizer accepted: it holds the port to digits, which keeps the
    // authority well-formed, but not to a range -- and `:99999` is a number no
    // socket has, so the server that would fetch this document never gets as
    // far as trying.
    let parsed = match Url::parse(&canonical) {
        Ok(parsed) => parsed,
        Err(ParseError::InvalidPort) => return invalid("must name a port in the 0-65535 range"),
        Err(err) => return invalid(&format!("is not a valid URL: {err}")),
    };

    match parsed.scheme() {
        "https" => {}
        "http" if !require_https => {}
        "http" => {
            return invalid("must use the `https` scheme (or set `require_https(false)`)");
        }
        _ => return invalid("must be an absolute `https` URL"),
    }

    // A path is a path, and a query is a query: reading the two off the parse
    // is what keeps `https://example.com?location=/client.json` from passing on
    // the strength of the slash inside its query. `Url` gives every http(s) URL
    // a path of at least `/`, so the bare origin -- which as a client id would
    // make every client hosted there the same client -- arrives here as exactly
    // that, however it was spelled.
    if matches!(parsed.path(), "" | "/") {
        return invalid("must contain a path component, e.g. `https://example.com/client.json`");
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
    /// What names this identity in a [`TokenStore`] key, across restarts.
    ///
    /// A dynamically registered client names nothing: the id it is about to be
    /// given is not the one it had last time, so credentials obtained under it
    /// belong to no identity that outlives the flow. They still get a slot --
    /// the warm start reads it -- but an unnamed one, which
    /// [`OAuthSession::may_reuse_stored_refresh`] never renews from.
    fn persistent_id(&self) -> &str {
        match self {
            Self::PreRegistered(client_id) => client_id,
            Self::Document(url) => url,
            Self::Dynamic => "",
        }
    }

    /// Whether this identity is the same on the next run of the process.
    ///
    /// A dynamically registered id is not: the next run registers again and
    /// gets another one, which is why credentials tied to the old id -- a
    /// stored refresh token above all -- are worthless after a restart.
    fn survives_a_restart(&self) -> bool {
        !matches!(self, Self::Dynamic)
    }
}

/// Whether `uri` redirects to a loopback interface, per the native-client
/// loopback exception.
///
/// RFC 8252 section 7.3 gives a native client the whole of `127.0.0.0/8`, not
/// just `127.0.0.1` -- a handler bound to `127.0.0.2` redirects to loopback
/// just as much, and calling it a `web` client would have an OIDC-strict
/// authorization server refuse the plain-http redirect URI. `Host` decides
/// that from a parsed address rather than from a string comparison, so the
/// range comes for free and `localhost` stays the one name that counts.
fn is_loopback_redirect(uri: &str) -> bool {
    let Ok(url) = Url::parse(uri) else {
        return false;
    };

    // The exception is about http(s) redirects; a custom scheme claiming the
    // loopback host is a private-use URI redirect, which is a different
    // mechanism with its own rules.
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }

    match url.host() {
        Some(Host::Domain(host)) => host == "localhost",
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
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
/// Returns `None` when `resource` is not a URL with an authority -- an opaque
/// origin has no host to hang a well-known path off, and neither does a string
/// that does not parse.
fn origin_of(resource: &str) -> Option<String> {
    let origin = Url::parse(resource).ok()?.origin();

    origin.is_tuple().then(|| origin.ascii_serialization())
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
    fn loopback_redirects_are_detected() {
        assert!(is_loopback_redirect("http://127.0.0.1:8919/callback"));
        assert!(is_loopback_redirect("http://localhost/callback"));
        assert!(is_loopback_redirect("http://[::1]:9000/callback"));
        assert!(!is_loopback_redirect("https://my.app/oauth/callback"));
        assert!(!is_loopback_redirect("res://localhost"));
    }

    /// RFC 8252 section 7.3 hands a native client the whole loopback range, so
    /// a handler bound anywhere in `127.0.0.0/8` is native too. Matching the
    /// literal `127.0.0.1` registered those as `web` clients, which an
    /// OIDC-strict authorization server refuses for a plain-http redirect URI.
    #[test]
    fn the_whole_loopback_range_counts_as_loopback() {
        assert!(is_loopback_redirect("http://127.0.0.2:8919/callback"));
        assert!(is_loopback_redirect("http://127.1.2.3/callback"));
        assert!(is_loopback_redirect("http://[::0:0:1]:9000/callback"));
        // Neighbouring ranges are not loopback: `127.0.0.0/8` ends at 127.
        assert!(!is_loopback_redirect("http://128.0.0.1/callback"));
        assert!(!is_loopback_redirect("http://126.255.255.255/callback"));
        // A name that merely looks like one. Only `localhost` is the loopback
        // name, and `url` will not resolve anything else to prove otherwise.
        assert!(!is_loopback_redirect("http://localhost.evil.com/callback"));
        assert!(!is_loopback_redirect("http://not-a-url"));
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

    /// The store key a credential with this identity is filed under.
    fn key(issuer: &str, client: &str, resource: &str) -> String {
        format!("{issuer}|{client}|{resource}")
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
        // The scheme is case-insensitive, and both the canonicalizer and the
        // parser normalize it -- so the check never rests on the spelling.
        assert!(validate_client_id_document_url("HTTPS://Example.COM/c.json", true).is_ok());

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
            &*session.store_key_for(&moved.issuer, ClientIdSource::Document(CIMD_URL)),
            &*session.store_key(),
            "and not from the slot the stale configuration names"
        );
        assert!(
            session
                .store_key_for(&moved.issuer, ClientIdSource::Document(CIMD_URL))
                .starts_with("https://other.example.com|"),
            "the slot is the one that server files its own tokens in"
        );
    }

    /// Two clients are two grants: the user consented to each separately, for
    /// scopes they chose separately. Sharing one durable store -- an encrypted
    /// file, a keychain -- must not have them share a slot, or the second
    /// client sends the first one's access token as if the consent behind it
    /// were its own.
    #[test]
    fn two_client_identities_do_not_share_a_slot() {
        const RESOURCE: &str = "https://api.example.com/mcp";
        let store: Arc<dyn TokenStore> = Arc::new(InMemoryTokenStore::new());

        let config = |document: &str| OAuthClientConfig {
            store: store.clone(),
            ..OAuthClientConfig::default()
                .with_client_id_document(document)
                .with_issuer("https://auth.example.com")
        };

        let first = OAuthSession::new(config(CIMD_URL), RESOURCE).unwrap();
        store.put(
            &first.store_key(),
            &TokenSet {
                access_token: "the-first-clients-token".into(),
                token_type: "Bearer".into(),
                refresh_token: Some("the-first-clients-refresh".into()),
                scope: None,
                id_token: None,
                expires_at: None,
            },
        );

        // Same resource, same authorization server, a different client.
        let second =
            OAuthSession::new(config("https://other.example.com/client.json"), RESOURCE).unwrap();

        assert_ne!(&*first.store_key(), &*second.store_key());
        assert_eq!(
            second.bearer(),
            None,
            "a client must not start out holding another client's token"
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
            store_key: RwLock::new(key("", "", "http://127.0.0.1:3000/mcp").into()),
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
        store.put(&key("", "", "http://127.0.0.1:3000/mcp"), &stale_tokens());

        let flow = FlowState {
            client: OAuthClient::new("cid")
                .with_config(ClientConfig::new().require_https(false))
                .with_token_store(store.clone()),
            metadata: AuthorizationServerMetadata::new("http://issuer.local")
                .with_token_endpoint(format!("http://{addr}/token")),
            store_key: key("", "", "http://127.0.0.1:3000/mcp").into(),
        };
        let session = session_with(store.clone(), Some(flow));

        let token = session.refreshed_bearer().await;

        assert_eq!(token.as_deref(), Some("fresh-token"));
        let stored = store
            .get(&key("", "", "http://127.0.0.1:3000/mcp"))
            .unwrap();
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
        store.put(&key("", "", "http://127.0.0.1:3000/mcp"), &restored);

        let flow = FlowState {
            client: OAuthClient::new("cid")
                .with_config(ClientConfig::new().require_https(false))
                .with_token_store(store.clone()),
            metadata: AuthorizationServerMetadata::new("http://issuer.local")
                .with_token_endpoint(format!("http://{addr}/token")),
            store_key: key("", "", "http://127.0.0.1:3000/mcp").into(),
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
                .get(&key("", "", "http://127.0.0.1:3000/mcp"))
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
        store.put(&key("", "", "http://127.0.0.1:3000/mcp"), &restored);

        let flow = FlowState {
            client: OAuthClient::new("cid")
                .with_config(ClientConfig::new().require_https(false))
                .with_token_store(store.clone()),
            metadata: AuthorizationServerMetadata::new("http://issuer.local")
                .with_token_endpoint(format!("http://{addr}/token")),
            store_key: key("", "", "http://127.0.0.1:3000/mcp").into(),
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
                .get(&key("", "", "http://127.0.0.1:3000/mcp"))
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
        store.put(&key("", "", "http://127.0.0.1:3000/mcp"), &tokens);

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
        store.put(&key("", "", "http://127.0.0.1:3000/mcp"), &stale_tokens());

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
            &key("", "", "http://127.0.0.1:3000/mcp"),
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
                &key("", "", "http://127.0.0.1:3000/mcp"),
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
            store.get(&key(stale_config, CIMD_URL, &resource)).is_none(),
            "nothing may be filed under a server that minted none of it"
        );
        assert_eq!(
            store
                .get(&key(&format!("http://{addr}"), CIMD_URL, &resource))
                .map(|tokens| tokens.access_token),
            Some("cimd-token".to_owned()),
            "the tokens belong to the server the flow actually ran against"
        );
        assert_eq!(
            &*session.store_key(),
            key(&format!("http://{addr}"), CIMD_URL, &resource),
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
        store.put(
            &key(&format!("http://{addr}"), CIMD_URL, &resource),
            &stale_tokens(),
        );

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

    /// A document configured against a server that does not resolve one falls
    /// back to registration -- and what that registration obtains belongs to a
    /// throwaway client, not to the document. Filed under the document, it
    /// would be read back as the document's own by a later flow (against a
    /// server that has since enabled documents, say) and presented under a
    /// client id that never held it: `invalid_grant`, the entry discarded, and
    /// the user asked again for no reason.
    #[tokio::test]
    async fn a_fallback_registration_is_not_filed_under_the_document() {
        let addr = spawn_registering_authorization_server().await;
        let resource = format!("http://{addr}/mcp");
        let issuer = format!("http://{addr}");

        let store: Arc<dyn TokenStore> = Arc::new(InMemoryTokenStore::new());
        let config = OAuthClientConfig {
            store: store.clone(),
            ..OAuthClientConfig::default()
                .require_https(false)
                // Configured, but this server advertises no support for it and
                // does offer a registration endpoint.
                .with_client_id_document(CIMD_URL)
                .with_issuer(&issuer)
                .with_handler(EchoesState)
        };
        let session = OAuthSession::new(config, &resource).unwrap();

        let token = session.authorize(None, None).await.expect("the flow runs");
        assert_eq!(&*token, "granted-token");

        assert!(
            store.get(&key(&issuer, CIMD_URL, &resource)).is_none(),
            "a registered client's tokens must not be filed under the document"
        );
        assert_eq!(
            store
                .get(&key(&issuer, "", &resource))
                .map(|tokens| tokens.access_token),
            Some("granted-token".to_owned()),
            "they belong to an identity that outlives nothing, and are filed as such"
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
                .get(&key("", "", &resource))
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
        // Under the identity that obtained it, which is where the next run
        // looks: the server that minted it and the client it was issued to.
        store.put(
            &key(&format!("http://{addr}"), "cid", &resource),
            &stale_tokens(),
        );

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
        store.put(&key("", "cid", &resource), &stale_tokens());

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
        store.put(&key(previous_issuer, "cid", &resource), &stale_tokens());

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
                .get(&key(previous_issuer, "cid", &resource))
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
            &key("", "", "http://127.0.0.1:9/mcp"),
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
            &key("", "", RESOURCE),
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
            &key("", "", RESOURCE),
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

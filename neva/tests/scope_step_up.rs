//! A tool gated on a scope the caller's token lacks, end to end: the server
//! answers `403` with an `insufficient_scope` challenge, the neva client
//! re-authorizes for the union of what it holds and what the challenge
//! demands (SEP-2350), and the retry goes through.
//!
//! The server is a real neva server validating HS256 JWTs. The authorization
//! server is a socket that mints them, for whatever scope the authorization
//! request asked for.
#![cfg(all(
    feature = "http-server-volga",
    feature = "server-oauth",
    feature = "client-oauth"
))]

mod common;

use neva::{
    App,
    auth::oauth::{AuthorizationHandler, CallbackParams},
    client::Client,
    error::{Error, ErrorCode},
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use volga::auth::{BearerAuthConfig, BearerTokenService, EncodingKey};

const SECRET: &[u8] = b"a-string-secret-at-least-256-bits-long";

/// The `scope` of each authorization request, in order. The authorization
/// code handed back is the index into this list, which is how the token
/// endpoint knows what to grant.
#[derive(Clone, Default)]
struct Consent(Arc<Mutex<Vec<String>>>);

impl Consent {
    fn asked(&self) -> Vec<BTreeSet<String>> {
        self.0
            .lock()
            .expect("consent")
            .iter()
            .map(|scope| scope.split(' ').map(str::to_owned).collect())
            .collect()
    }

    fn granted(&self, code: &str) -> Option<String> {
        let index = code.parse::<usize>().ok()?;
        self.0.lock().expect("consent").get(index).cloned()
    }
}

impl AuthorizationHandler for Consent {
    async fn redirect_uri(&self) -> Result<String, Error> {
        Ok("http://127.0.0.1:1/callback".into())
    }

    async fn authorize(&self, url: String) -> Result<CallbackParams, Error> {
        let query = url.split_once('?').map(|(_, query)| query).unwrap_or("");
        let param = |name: &str| {
            url::form_urlencoded::parse(query.as_bytes())
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };
        let state = param("state").ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidRequest,
                "the authorization URL had no state",
            )
        })?;

        let mut asked = self.0.lock().expect("consent");
        asked.push(param("scope").unwrap_or_default());
        Ok(CallbackParams {
            code: (asked.len() - 1).to_string(),
            state,
            iss: None,
        })
    }
}

/// An HS256 access token for `resource`, granting `scope`.
fn token(resource: &str, scope: &str) -> String {
    let exp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
        + 3600;
    let service = BearerTokenService::from(
        BearerAuthConfig::default().set_encoding_key(EncodingKey::from_secret(SECRET)),
    );
    service
        .encode(&json!({ "sub": "alice", "aud": resource, "scope": scope, "exp": exp }))
        .expect("token")
        .to_string()
}

/// Reads one HTTP request off `stream`: the head, and as much body as its
/// `Content-Length` says.
async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = stream.read(&mut chunk).await.unwrap_or(0);
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..read]);

        let text = String::from_utf8_lossy(&buf);
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.trim()
                        .eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            if body.len() >= length {
                break;
            }
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Serves the authorization server's metadata, registration and token
/// endpoints. Each token grants the scope its code was issued for.
async fn authorization_server(consent: Consent, resource: String) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let root = format!("http://{}", listener.local_addr().expect("addr"));

    let issuer = root.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let request = read_request(&mut stream).await;
            let body = if request.starts_with("POST /token") {
                let form = request.split_once("\r\n\r\n").map_or("", |(_, body)| body);
                let scope = url::form_urlencoded::parse(form.as_bytes())
                    .find(|(key, _)| key == "code")
                    .and_then(|(_, code)| consent.granted(&code))
                    .unwrap_or_default();
                json!({
                    "access_token": token(&resource, &scope),
                    "token_type": "Bearer",
                    "expires_in": 3600,
                    "scope": scope,
                })
            } else if request.starts_with("POST /register") {
                json!({ "client_id": "registered-client" })
            } else {
                json!({
                    "issuer": issuer,
                    "authorization_endpoint": format!("{issuer}/authorize"),
                    "token_endpoint": format!("{issuer}/token"),
                    "registration_endpoint": format!("{issuer}/register"),
                    "response_types_supported": ["code"],
                    "code_challenge_methods_supported": ["S256"],
                })
            }
            .to_string();

            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes()).await;
        }
    });

    root
}

/// A server at `addr` with `recall` gated on `memory:read` and `forget` on
/// `memory:write`, advertising `scopes` -- or, when `None`, what its tools
/// declare.
///
/// The address is picked by the caller: the tokens need it as their audience
/// before the server is up.
async fn serve(addr: &str, issuer: &str, scopes: Option<&[&str]>) -> tokio::task::JoinHandle<()> {
    let resource = format!("http://{addr}/mcp");
    let issuer = issuer.to_owned();
    let scopes = scopes.map(|scopes| scopes.iter().map(|s| s.to_string()).collect::<Vec<_>>());

    let mut app = App::new().without_greeting().with_options(|o| {
        o.with_http(|h| {
            h.bind(addr)
                .with_endpoint("/mcp")
                .with_auth(|auth| auth.set_decoding_key(SECRET).with_aud([resource.as_str()]))
                .with_oauth_metadata(|oauth| {
                    let oauth = oauth.with_authorization_servers([issuer]);
                    match scopes {
                        Some(scopes) => oauth.with_scopes(scopes),
                        None => oauth,
                    }
                })
        })
    });

    app.map_tool("recall", || async { "remembered" })
        .with_scopes(["memory:read"]);
    app.map_tool("forget", || async { "forgotten" })
        .with_scopes(["memory:write"]);

    let server = tokio::spawn(async move { app.run().await });
    common::serving(addr, &server).await;
    server
}

fn text(result: &impl serde::Serialize) -> String {
    let result = serde_json::to_value(result).expect("serialize");
    result
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no text in {result}"))
        .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_short_of_a_scope_steps_up_and_the_call_goes_through() {
    let consent = Consent::default();
    let addr = format!("127.0.0.1:{}", common::free_port());
    let issuer = authorization_server(consent.clone(), format!("http://{addr}/mcp")).await;

    // Advertising only `memory:read` is what makes the first token too
    // narrow for `forget`.
    let server = serve(&addr, &issuer, Some(&["memory:read"])).await;

    let mut client = Client::new().with_options(|o| {
        o.with_http(|h| {
            h.bind(&addr)
                .with_endpoint("/mcp")
                .with_oauth(|oauth| oauth.require_https(false).with_handler(consent.clone()))
        })
        .with_timeout(Duration::from_secs(10))
    });
    client.connect().await.expect("connect");

    let recalled = client.tools().call("recall", ()).await.expect("recall");
    assert_eq!(text(&recalled), "remembered");

    let forgotten = client.tools().call("forget", ()).await.expect("forget");
    assert_eq!(text(&forgotten), "forgotten");

    let set = |scopes: &[&str]| {
        scopes
            .iter()
            .map(|s| s.to_string())
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(
        consent.asked(),
        [set(&["memory:read"]), set(&["memory:read", "memory:write"])],
        "one authorization up front, then one step-up asking for the union"
    );

    // The wider token still covers what the first one did.
    let recalled = client.tools().call("recall", ()).await.expect("recall");
    assert_eq!(text(&recalled), "remembered");
    assert_eq!(consent.asked().len(), 2, "no further authorization");

    server.abort();
}

/// A client that cannot step up -- a static token -- is handed the error
/// itself, with the scopes in its `data`.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_cannot_step_up_gets_the_insufficient_scope_error() {
    let addr = format!("127.0.0.1:{}", common::free_port());
    let resource = format!("http://{addr}/mcp");
    let server = serve(&addr, "https://auth.example.com", None).await;

    let mut client = Client::new().with_options(|o| {
        o.with_http(|h| {
            h.bind(&addr)
                .with_endpoint("/mcp")
                .with_auth(token(&resource, "memory:read"))
        })
        .with_timeout(Duration::from_secs(10))
    });
    client.connect().await.expect("connect");

    let err = client
        .tools()
        .call("forget", ())
        .await
        .expect_err("refused");
    assert_eq!(err.code(), ErrorCode::InsufficientScope);
    assert!(err.to_string().contains("memory:write"), "{err}");
    assert_eq!(
        err.data(),
        Some(&json!({
            "requiredScopes": ["memory:write"],
            "missingScopes": ["memory:write"],
        }))
    );

    server.abort();
}

/// Without `with_scopes`, `scopes_supported` is every scope the server's
/// tools declare, so the two cannot drift apart.
#[tokio::test(flavor = "multi_thread")]
async fn scopes_supported_lists_what_the_tools_declare() {
    let addr = format!("127.0.0.1:{}", common::free_port());
    let server = serve(&addr, "https://auth.example.com", None).await;

    let doc: Value = reqwest::get(format!(
        "http://{addr}/.well-known/oauth-protected-resource/mcp"
    ))
    .await
    .expect("metadata")
    .json()
    .await
    .expect("json");
    assert_eq!(
        doc["scopes_supported"],
        json!(["memory:read", "memory:write"])
    );

    server.abort();
}

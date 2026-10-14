//! An authorization server's tokens, validated by a server that cannot reach
//! the URL they name as their issuer (#163): with the issuer's public key
//! configured statically, and with its key set fetched from an explicit
//! `jwks_uri` instead of discovered from the issuer.
//!
//! The tokens are real ES256 JWTs. The issuer they name is a port nothing
//! listens on, so a server that tried to reach it would get no keys.
#![cfg(all(feature = "http-server-volga", feature = "http-client"))]

mod common;

use neva::{
    App, Context,
    auth::{Algorithm, AuthConfig, DecodingKey},
    client::Client,
};
use serde_json::{Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// What the tokens carry in `iss`. Nothing listens there.
const ISSUER: &str = "http://127.0.0.1:1";

/// The issuer's signing key: a throwaway P-256 key made for this test.
const SIGNING_KEY: &[u8] = b"-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgsKzGOz3zWRc6z6gH
yFVq5xo1qWePGdLDUIqUCuRIeIihRANCAAR6AlqtBCCzNWq9DFfQNb87rdCRJrmZ
Lnq97JYIfiQKa60KaIeeDHKBadSzKZOSmdKkdct1rLHkxDgbaPFbGTQ9
-----END PRIVATE KEY-----
";

/// The public half of [`SIGNING_KEY`], as PEM.
const PUBLIC_KEY: &[u8] = b"-----BEGIN PUBLIC KEY-----
MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEegJarQQgszVqvQxX0DW/O63QkSa5
mS56veyWCH4kCmutCmiHngxygWnUsymTkpnSpHXLdayx5MQ4G2jxWxk0PQ==
-----END PUBLIC KEY-----
";

/// The public half of [`SIGNING_KEY`], as a JSON Web Key.
const PUBLIC_JWK: &str = r#"{"kty":"EC","crv":"P-256","x":"egJarQQgszVqvQxX0DW_O63QkSa5mS56veyWCH4kCms","y":"rQpoh54McoFp1LMpk5KZ0qR1y3WsseTEOBto8VsZND0"}"#;

/// Another throwaway P-256 key, one the server does not trust.
const OTHER_SIGNING_KEY: &[u8] = b"-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgnRP8v22PUggPb6gR
3XLAEkLEBTR4F8nO7w2vMTDb/SqhRANCAAShm6vtDqwdUIIwTrsO1xjDxny1ik3Y
yGroiVumfxaYlIcC7QPDvA0rvenVT8J4an+3wKeI1WYbp6hR6fZFW+C0
-----END PRIVATE KEY-----
";

/// A server at `addr` validating bearer tokens as `auth` configures it,
/// with one tool that answers who is calling.
async fn serve(
    addr: &str,
    auth: impl FnOnce(AuthConfig) -> AuthConfig,
) -> tokio::task::JoinHandle<()> {
    let mut app = App::new()
        .without_greeting()
        .with_options(|o| o.with_http(|h| h.bind(addr).with_endpoint("/mcp").with_auth(auth)));

    app.map_tool("whoami", |ctx: Context| async move {
        ctx.claims()
            .and_then(|c| c.subject())
            .unwrap_or("nobody")
            .to_owned()
    });

    let server = tokio::spawn(async move { app.run().await });
    common::serving(addr, &server).await;
    server
}

/// An ES256 token over `claims` signed with `key`, valid for the next hour.
fn sign(key: &[u8], mut claims: Value) -> String {
    let exp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
        + 3600;
    claims["exp"] = json!(exp);

    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256),
        &claims,
        &jsonwebtoken::EncodingKey::from_ec_pem(key).expect("signing key"),
    )
    .expect("token")
}

/// The issuer's token for `aud`, signed with `key`.
fn token(key: &[u8], aud: &str) -> String {
    sign(key, json!({ "sub": "alice", "iss": ISSUER, "aud": aud }))
}

/// Who the server says is calling, on a tool call with `token`.
async fn whoami(addr: &str, token: String) -> String {
    let mut client = Client::new().with_options(|o| {
        o.with_http(|h| h.bind(addr).with_endpoint("/mcp").with_auth(token))
            .with_timeout(Duration::from_secs(10))
    });
    client.connect().await.expect("connect");

    let result = client.tools().call("whoami", ()).await.expect("call");
    let result = serde_json::to_value(&result).expect("serialize");
    result
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no text in {result}"))
        .to_owned()
}

/// The status the server answers a request carrying `token` with.
async fn status(addr: &str, token: &str) -> u16 {
    reqwest::Client::new()
        .post(format!("http://{addr}/mcp"))
        .bearer_auth(token)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)
        .send()
        .await
        .expect("request")
        .status()
        .as_u16()
}

/// A server holding `key` takes the issuer's tokens, and refuses the ones
/// another key signed and the ones that name no issuer.
async fn validates_with(key: DecodingKey) {
    let addr = format!("127.0.0.1:{}", common::free_port());
    let resource = format!("http://{addr}/mcp");
    let server = serve(&addr, |auth| {
        auth.with_decoding_key(key)
            .with_alg(Algorithm::ES256)
            .with_iss([ISSUER])
            .with_aud([resource.as_str()])
    })
    .await;

    assert_eq!(whoami(&addr, token(SIGNING_KEY, &resource)).await, "alice");
    assert_eq!(
        status(&addr, &token(OTHER_SIGNING_KEY, &resource)).await,
        401
    );
    // `with_iss` requires the claim, rather than checking it when present
    let no_issuer = sign(SIGNING_KEY, json!({ "sub": "alice", "aud": resource }));
    assert_eq!(status(&addr, &no_issuer).await, 401);

    server.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_public_key_from_pem_validates_the_issuers_tokens() {
    validates_with(DecodingKey::from_ec_pem(PUBLIC_KEY).expect("public key")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_public_key_from_a_jwk_validates_the_issuers_tokens() {
    validates_with(DecodingKey::from_jwk(PUBLIC_JWK).expect("public key")).await;
}

/// Serves the issuer's key set, at an address other than the issuer's own,
/// and answers with its URL.
#[cfg(feature = "server-oauth")]
async fn jwks_server() -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let url = format!("http://{}/jwks", listener.local_addr().expect("addr"));
    let body = format!(r#"{{"keys":[{PUBLIC_JWK}]}}"#);

    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            // a GET has no body: the head is all there is to read
            let mut head = Vec::new();
            let mut chunk = [0u8; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => head.extend_from_slice(&chunk[..read]),
                }
            }
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes()).await;
        }
    });

    url
}

#[cfg(feature = "server-oauth")]
#[tokio::test(flavor = "multi_thread")]
async fn keys_come_from_an_explicit_jwks_uri() {
    let jwks = jwks_server().await;
    let addr = format!("127.0.0.1:{}", common::free_port());
    // the audience OAuth issuer mode binds tokens to by default
    let resource = format!("http://{addr}/mcp");
    let server = serve(&addr, |auth| {
        auth.with_oauth(|oauth| {
            oauth
                .with_issuer(ISSUER)
                .with_jwks_uri(jwks)
                .with_config(|cfg| cfg.with_client_config(|c| c.require_https(false)))
        })
    })
    .await;

    assert_eq!(whoami(&addr, token(SIGNING_KEY, &resource)).await, "alice");
    assert_eq!(
        status(&addr, &token(OTHER_SIGNING_KEY, &resource)).await,
        401
    );

    server.abort();
}

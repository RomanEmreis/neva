//! Who is calling, as a handler and a middleware see it: the claims of the
//! bearer token Volga validated, read through `claims()` on `Context` and on
//! `MwContext`.
//!
//! The token is a real HS256 JWT checked against an audience and an issuer, so
//! this also covers a token whose `aud` is an array (RFC 8707), which the
//! claims used to be unable to decode.
#![cfg(all(feature = "http-server-volga", feature = "http-client"))]

mod common;

use neva::{App, Context, client::Client};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use volga::auth::{BearerAuthConfig, BearerTokenService, EncodingKey};

const SECRET: &[u8] = b"a-string-secret-at-least-256-bits-long";
const ISSUER: &str = "https://auth.example.com";
const AUDIENCE: &str = "https://mcp.example.com/mcp";
const AGENT: &str = "memory-agent";

/// The `client_id` of each tool call, as the middleware saw it.
type Seen = Arc<Mutex<Vec<Option<String>>>>;

async fn serve() -> (String, Seen, tokio::task::JoinHandle<()>) {
    let addr = format!("127.0.0.1:{}", common::free_port());
    let seen = Seen::default();

    let mut app = App::new()
        .without_greeting()
        .with_options(|o| {
            o.with_http(|h| {
                h.bind(&addr).with_endpoint("/mcp").with_auth(|auth| {
                    auth.with_aud([AUDIENCE])
                        .with_iss([ISSUER])
                        .set_decoding_key(SECRET)
                })
            })
        })
        .wrap_tools({
            let seen = seen.clone();
            move |ctx, next| {
                let client = ctx.claims().and_then(|c| c.client_id()).map(Into::into);
                seen.lock().expect("seen").push(client);
                next(ctx)
            }
        });

    app.map_tool("whoami", |ctx: Context| async move {
        let caller = ctx.claims();
        json!({
            "subject": caller.and_then(|c| c.subject()),
            "issuer": caller.and_then(|c| c.issuer()),
            "audience": caller.map(|c| c.audience()),
            "client_id": caller.and_then(|c| c.client_id()),
            "scopes": caller.map(|c| c.scopes()),
        })
        .to_string()
    });

    let server = tokio::spawn(async move { app.run().await });
    common::serving(&addr, &server).await;
    (addr, seen, server)
}

/// An HS256 token over `claims`, valid for the next hour.
fn token(mut claims: Value) -> String {
    let exp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
        + 3600;
    claims["exp"] = json!(exp);

    let service = BearerTokenService::from(
        BearerAuthConfig::default().set_encoding_key(EncodingKey::from_secret(SECRET)),
    );
    service.encode(&claims).expect("token").to_string()
}

async fn connect(addr: &str, token: String) -> Client {
    let mut client = Client::new().with_options(|o| {
        o.with_http(|h| h.bind(addr).with_endpoint("/mcp").with_auth(token))
            .with_timeout(Duration::from_secs(10))
    });
    client.connect().await.expect("connect");
    client
}

#[tokio::test(flavor = "multi_thread")]
async fn a_handler_reads_who_is_calling() {
    let (addr, _, server) = serve().await;
    let client = connect(
        &addr,
        token(json!({
            "sub": "alice",
            "iss": ISSUER,
            "aud": [AUDIENCE, "https://api.example.com"],
            "client_id": AGENT,
            "scope": "memory:read memory:write",
        })),
    )
    .await;

    let result = client.tools().call("whoami", ()).await.expect("call");
    let result = serde_json::to_value(&result).expect("serialize");
    let text = result
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no text in {result}"));
    let caller: Value = serde_json::from_str(text).expect("json");

    assert_eq!(
        caller,
        json!({
            "subject": "alice",
            "issuer": ISSUER,
            "audience": [AUDIENCE, "https://api.example.com"],
            "client_id": AGENT,
            "scopes": ["memory:read", "memory:write"],
        })
    );

    server.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_middleware_reads_the_same_claims() {
    let (addr, seen, server) = serve().await;
    let client = connect(
        &addr,
        token(json!({
            "sub": "alice",
            "iss": ISSUER,
            "aud": AUDIENCE,
            "client_id": AGENT,
        })),
    )
    .await;

    client.tools().call("whoami", ()).await.expect("call");

    assert_eq!(*seen.lock().expect("seen"), [Some(AGENT.to_string())]);

    server.abort();
}

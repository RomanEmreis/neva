//! A middleware that answers a request without calling `next`: what it returns
//! reaches the caller over HTTP, on its own and inside a batch. It used to be
//! dropped, and the caller waited until its timeout.
#![cfg(all(feature = "http-server-volga", feature = "http-client"))]

mod common;

use neva::{
    App,
    client::Client,
    error::{Error, ErrorCode},
    types::Response,
};
use std::time::Duration;

const REFUSED: &str = "refused by middleware";

async fn serve() -> (String, tokio::task::JoinHandle<()>) {
    let addr = format!("127.0.0.1:{}", common::free_port());

    let mut app = App::new()
        .without_greeting()
        .with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")));
    app.map_tool("ping", || async { "pong" });
    app.map_tool("forbidden", || async { "never answered" });
    app.wrap_tool("forbidden", |ctx, _next| async move {
        Response::error(ctx.id(), Error::new(ErrorCode::InvalidRequest, REFUSED))
    });

    let server = tokio::spawn(async move { app.run().await });
    common::serving(&addr, &server).await;
    (addr, server)
}

/// Well under the time a dropped answer took to surface as a timeout.
async fn connect(addr: &str) -> Client {
    let mut client = Client::new().with_options(|o| {
        o.with_http(|h| h.bind(addr).with_endpoint("/mcp"))
            .with_timeout(Duration::from_secs(5))
    });
    client.connect().await.expect("connect");
    client
}

#[tokio::test(flavor = "multi_thread")]
async fn the_caller_gets_the_middleware_answer() {
    let (addr, server) = serve().await;
    let client = connect(&addr).await;

    let err = client
        .tools()
        .call("forbidden", ())
        .await
        .expect_err("the middleware refuses the call");
    assert!(format!("{err}").contains(REFUSED), "{err}");

    // The calls the middleware lets through are answered as before.
    let result = client.tools().call("ping", ()).await.expect("ping");
    let result = serde_json::to_string(&result).expect("serialize");
    assert!(result.contains("pong"), "{result}");

    server.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_batch_gets_the_middleware_answer_in_its_place() {
    let (addr, server) = serve().await;
    let client = connect(&addr).await;

    let responses = client
        .batch()
        .call_tool("forbidden", ())
        .call_tool("ping", ())
        .send()
        .await
        .expect("batch");

    let [refused, answered] = responses.as_slice() else {
        panic!("two responses expected, got {responses:?}");
    };
    let Response::Err(refused) = refused else {
        panic!("the refused call must be an error, got {refused:?}");
    };
    assert_eq!(refused.error.message, REFUSED);
    assert!(
        matches!(answered, Response::Ok(_)),
        "the other call must be answered, got {answered:?}"
    );

    server.abort();
}

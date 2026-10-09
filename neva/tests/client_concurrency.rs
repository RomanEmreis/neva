//! One client, many callers.
//!
//! Requests take `&self`, so a connected client can be shared -- behind an
//! `Arc`, across tasks -- and its calls are in flight together rather than one
//! after another.
#![cfg(all(feature = "http-server-volga", feature = "http-client"))]

mod common;

use neva::{App, client::Client};
use std::{sync::Arc, time::Duration};
use tokio::sync::Barrier;

const CALLERS: usize = 8;

/// Every call waits at a barrier sized to all of them, so the tool can only
/// answer once every call is in flight at the same time. Calls that went one
/// after another would never fill it, and the test would run into its timeout
/// rather than depend on how fast anything is.
#[tokio::test(flavor = "multi_thread")]
async fn calls_from_many_tasks_are_in_flight_together() {
    let addr = format!("127.0.0.1:{}", common::free_port());

    let barrier = Arc::new(Barrier::new(CALLERS));
    let mut app = App::new()
        .without_greeting()
        .with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")));
    app.map_tool("meet", move |name: String| {
        let barrier = barrier.clone();
        async move {
            barrier.wait().await;
            format!("met {name}")
        }
    })
    .with_arg_names(["name"]);
    let server = tokio::spawn(async move { app.run().await });
    common::serving(&addr, &server).await;

    let mut client = Client::new().with_options(|o| {
        o.with_http(|h| h.bind(&addr).with_endpoint("/mcp"))
            .with_timeout(Duration::from_secs(10))
    });
    client.connect().await.expect("connect");
    let client = Arc::new(client);

    let calls = (0..CALLERS).map(|i| {
        let client = client.clone();
        tokio::spawn(async move {
            let name = format!("caller-{i}");
            let resp = client
                .tools()
                .call("meet", [("name", name.as_str())])
                .await
                .expect("call");
            (name, resp)
        })
    });

    let results = tokio::time::timeout(
        Duration::from_secs(5),
        futures_util::future::join_all(calls),
    )
    .await
    .expect("all calls must be in flight at once to pass the barrier");

    for result in results {
        let (name, resp) = result.expect("task");
        let text = serde_json::to_string(&resp).expect("serialize");
        assert!(text.contains(&format!("met {name}")), "{name}: {text}");
    }

    server.abort();
}

//! What a client says on the way out.
//!
//! Ending a connection is a transport act: the client closes it. Anything sent
//! alongside is a protocol message, and has to be one the spec defines --
//! `notifications/cancelled` names one in-flight request and its
//! `params.requestId` is required, so it is neither a goodbye nor valid without
//! params.
#![cfg(all(feature = "http-server-volga", feature = "http-client"))]

use neva::{
    App,
    client::Client,
    middleware::{MwContext, Next},
};
use std::sync::{Arc, Mutex};

#[tokio::test(flavor = "multi_thread")]
async fn disconnecting_sends_no_notification() {
    let port = pick_free_port();
    let addr = format!("127.0.0.1:{port}");

    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = seen.clone();

    let mut app = App::new()
        .with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")))
        .wrap_notification(move |ctx: MwContext, next: Next| {
            let recorder = recorder.clone();
            async move {
                if let neva::types::Message::Notification(n) = &ctx.msg
                    && let Ok(mut seen) = recorder.lock()
                {
                    seen.push(n.method.to_string());
                }
                next(ctx).await
            }
        });

    app.map_tool("ping", || async move { "pong".to_string() });

    let handle = tokio::spawn(async move { app.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Several cycles, because anything sent on the way out races the transport
    // cancellation that follows it and can lose. That race is also this test's
    // limit: on a machine where the cancellation always wins, a client that
    // still sends something passes here anyway. It costs little and it is the
    // only in-repo statement of the rule.
    for _ in 0..20 {
        let mut client = Client::new().with_options(|o| {
            o.with_http(|h| h.bind(&addr).with_endpoint("/mcp"))
                .with_timeout(std::time::Duration::from_secs(10))
        });
        client.connect().await.expect("connect");
        client.list_tools(None).await.expect("list tools");
        client.disconnect().await.expect("disconnect");
    }

    // The transport close needs a moment to land, and so would a notification.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let seen = seen.lock().expect("recorded notifications").clone();
    assert!(
        !seen.iter().any(|m| m == "notifications/cancelled"),
        "disconnect must not announce itself with a request cancellation: {seen:?}"
    );

    handle.abort();
}

fn pick_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

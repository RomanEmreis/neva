//! `neva::testing`: the server driven from a test with no process and no port.
//!
//! What matters here is that each flow goes through the same dispatch as on the
//! wire -- the MRTR rounds, the subscription stream -- over the in-memory pipe,
//! and that the harness reports a server that goes wrong instead of hanging.
#![cfg(feature = "testing")]

use neva::{App, Client, error::ErrorCode, testing::TestClient};
use std::time::Duration;

fn text(resp: &neva::types::CallToolResponse) -> Option<&str> {
    resp.content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.as_str())
}

#[tokio::test]
async fn a_tool_is_listed_and_called() {
    let mut app = App::new();
    app.map_tool("add", |a: i32, b: i32| async move { a + b })
        .with_arg_names(["a", "b"]);

    let client = TestClient::connect(app).await.expect("connect");

    let tools = client.tools().list(None).await.expect("tools/list");
    assert!(tools.tools.iter().any(|t| t.name == "add"));

    let resp = client
        .tools()
        .call("add", [("a", 2), ("b", 3)])
        .await
        .expect("tools/call");
    assert_eq!(text(&resp), Some("5"));

    client.shutdown().await.expect("shutdown");
}

/// The effects of an MRTR tool across its rounds: the handler re-runs, `memo`
/// and `once` hold, `on_commit` fires once, and the client answers the
/// elicitation with the handler it was configured with.
#[cfg(not(feature = "legacy-spec"))]
#[tokio::test]
async fn mrtr_elicitation_runs_end_to_end() {
    use neva::types::elicitation::{ElicitRequestParams, ElicitResult};
    use neva::{Context, error::Error};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static RUNS: AtomicUsize = AtomicUsize::new(0);
    static FETCHES: AtomicUsize = AtomicUsize::new(0);
    static CHARGES: AtomicUsize = AtomicUsize::new(0);
    static RECEIPTS: AtomicUsize = AtomicUsize::new(0);

    let mut app = App::new().with_request_state_secret(b"test-secret");
    app.map_tool("effectful", |ctx: Context| async move {
        RUNS.fetch_add(1, Ordering::SeqCst);
        let price: i32 = ctx
            .memo("quote", async {
                FETCHES.fetch_add(1, Ordering::SeqCst);
                Ok(42)
            })
            .await?;
        ctx.once("charge", async {
            CHARGES.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await?;
        ctx.on_commit(async {
            RECEIPTS.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        let ask: ElicitRequestParams = ElicitRequestParams::form("Your name?")
            .with_required("name", "string")
            .into();
        let res = ctx.elicit("name", ask).await?;
        let name = res
            .content
            .and_then(|c| c.get("name").and_then(|v| v.as_str().map(str::to_owned)))
            .unwrap_or_else(|| "stranger".into());
        Ok::<_, Error>(format!("hello {name}, charged at {price}"))
    });

    let mut client = Client::new();
    client.map_elicitation(|_params: ElicitRequestParams| async move {
        ElicitResult::accept().with_content(serde_json::json!({ "name": "octocat" }))
    });
    let client = TestClient::connect_with(app, client)
        .await
        .expect("connect");

    let resp = client
        .tools()
        .call("effectful", ())
        .await
        .expect("the call completes through the MRTR loop");

    assert_eq!(text(&resp), Some("hello octocat, charged at 42"));
    assert_eq!(
        RUNS.load(Ordering::SeqCst),
        2,
        "the handler re-runs per round"
    );
    assert_eq!(FETCHES.load(Ordering::SeqCst), 1, "memo computed once");
    assert_eq!(CHARGES.load(Ordering::SeqCst), 1, "once ran once");
    assert_eq!(RECEIPTS.load(Ordering::SeqCst), 1, "commit fired once");

    client.shutdown().await.expect("shutdown");
}

/// A `subscriptions/listen` stream over the pipe: the call returns once the
/// filter is acknowledged, a change made by a tool call reaches the handler
/// registered for it, and a server shutting down answers the stream gracefully.
#[cfg(not(feature = "legacy-spec"))]
#[tokio::test]
async fn a_subscription_delivers_and_ends_gracefully() {
    use neva::client::SubscriptionEnd;
    use neva::types::{SubscriptionFilter, Tool};
    use neva::{Context, error::Error};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let mut app = App::new().with_options(|opt| opt.with_tools(|t| t.with_list_changed()));
    app.map_tool("grow", |ctx: Context| async move {
        ctx.tools()
            .add(Tool::new("grown", || async { "ok" }))
            .await?;
        Ok::<_, Error>("grown".to_string())
    });

    // Registered before connecting: the running client reads its notification
    // handlers when it connects, so one added afterwards is only seen if
    // another handler (the `tracing` feature's) was registered first.
    let seen = Arc::new(AtomicUsize::new(0));
    let counter = seen.clone();
    let mut handlers = Client::new().with_options(|opt| opt.with_timeout(Duration::from_secs(5)));
    handlers.subscribe(neva::types::tool::commands::LIST_CHANGED, move |_| {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });

    let client = TestClient::connect_with(app, handlers)
        .await
        .expect("connect");

    let subscription = client
        .listen(SubscriptionFilter::new().with_tools_changed())
        .await
        .expect("listen");
    assert!(subscription.acknowledged().tools_list_changed);
    assert!(subscription.is_fully_honored());

    client.tools().call("grow", ()).await.expect("tools/call");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while seen.load(Ordering::SeqCst) == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        seen.load(Ordering::SeqCst),
        1,
        "the change reaches the handler"
    );

    client.shutdown_handle().shutdown();
    let ended = tokio::time::timeout(Duration::from_secs(10), subscription.closed())
        .await
        .expect("closed() must not hang once the server is shutting down");
    assert!(
        matches!(ended, SubscriptionEnd::Graceful(_)),
        "a subscription ended by shutdown closes gracefully, got {ended:?}"
    );

    client.shutdown().await.expect("shutdown");
}

/// A panicking handler is answered, not hung on, and the server keeps serving.
#[tokio::test]
async fn a_panicking_tool_is_an_internal_error() {
    let mut app = App::new();
    app.map_tool("boom", || async {
        let nothing = std::hint::black_box(None::<&str>);
        nothing.expect("on purpose")
    });
    app.map_tool("ping", || async { "pong" });

    let client = TestClient::connect_with(
        app,
        Client::new().with_options(|opt| opt.with_timeout(Duration::from_secs(5))),
    )
    .await
    .expect("connect");

    let err = client
        .tools()
        .call("boom", ())
        .await
        .expect_err("the panic is an error to the caller");
    assert_eq!(err.code(), ErrorCode::InternalError, "{err}");

    let resp = client
        .tools()
        .call("ping", ())
        .await
        .expect("still serving");
    assert_eq!(text(&resp), Some("pong"));

    client.shutdown().await.expect("shutdown");
}

/// A server that cannot start fails `connect` with its own panic, at once,
/// rather than leaving the handshake to time out.
#[tokio::test]
async fn a_server_that_panics_at_startup_fails_connect() {
    use futures_util::FutureExt;

    let mut app = App::new();
    // Two names for a handler that reads one argument: `run` refuses to start.
    app.map_tool("bad", |a: String| async move { a })
        .with_arg_names(["a", "b"]);

    let started = std::panic::AssertUnwindSafe(TestClient::connect(app))
        .catch_unwind()
        .await;

    let panic = started.expect_err("the server's panic is raised from connect");
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .unwrap_or_default();
    assert!(message.contains("`bad`"), "{message}");
}

/// How many tasks are alive on this test's runtime once the teardown has had
/// time to finish: waits until the count is back to `baseline`, or five
/// seconds have passed, and returns the count it stopped at.
async fn alive_after_teardown(baseline: usize) -> usize {
    let metrics = tokio::runtime::Handle::current().metrics();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let alive = metrics.num_alive_tasks();
        if alive <= baseline || tokio::time::Instant::now() >= deadline {
            return alive;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn app_with_ping() -> App {
    let mut app = App::new();
    app.map_tool("ping", || async { "pong" });
    app
}

/// A `TestClient` dropped without `shutdown` stops both ends: the client's
/// reader, writer, receive loop and signal watcher go with the server's.
#[tokio::test]
async fn a_dropped_test_client_leaves_no_task_behind() {
    let baseline = tokio::runtime::Handle::current()
        .metrics()
        .num_alive_tasks();

    for _ in 0..3 {
        let client = TestClient::connect(app_with_ping()).await.expect("connect");
        drop(client);
    }

    assert_eq!(alive_after_teardown(baseline).await, baseline);
}

/// `shutdown` ends every task either side started, the signal watchers that
/// `App::run` and `Client::connect` start included (#178).
#[tokio::test]
async fn a_shut_down_test_client_leaves_no_task_behind() {
    let baseline = tokio::runtime::Handle::current()
        .metrics()
        .num_alive_tasks();

    for _ in 0..3 {
        let client = TestClient::connect(app_with_ping()).await.expect("connect");
        client.shutdown().await.expect("shutdown");
    }

    assert_eq!(alive_after_teardown(baseline).await, baseline);
}

/// A handshake the server refuses fails `connect` with the client it started
/// stopped, and the server too.
#[tokio::test]
async fn a_refused_handshake_leaves_no_task_behind() {
    use neva::{
        error::Error,
        middleware::{MwContext, Next},
        types::Response,
    };

    let baseline = tokio::runtime::Handle::current()
        .metrics()
        .num_alive_tasks();

    // Answers every request, the handshake's included, with an error that is
    // no reason to fall back to the other handshake.
    let refusing = app_with_ping().wrap(|ctx: MwContext, _next: Next| async move {
        Response::error(
            ctx.id(),
            Error::new(ErrorCode::InternalError, "refused on purpose"),
        )
    });
    let err = TestClient::connect(refusing)
        .await
        .expect_err("the handshake is refused");
    assert_eq!(err.code(), ErrorCode::InternalError, "{err}");

    assert_eq!(alive_after_teardown(baseline).await, baseline);
}

/// A server that panics before the client connected: `connect` re-raises the
/// panic, and the client it started does not outlive it.
#[tokio::test]
async fn a_server_that_panics_at_startup_leaves_no_task_behind() {
    use futures_util::FutureExt;

    let baseline = tokio::runtime::Handle::current()
        .metrics()
        .num_alive_tasks();

    let mut app = App::new();
    app.map_tool("bad", |a: String| async move { a })
        .with_arg_names(["a", "b"]);
    let started = std::panic::AssertUnwindSafe(TestClient::connect(app))
        .catch_unwind()
        .await;
    assert!(
        started.is_err(),
        "the server's panic is raised from connect"
    );

    assert_eq!(alive_after_teardown(baseline).await, baseline);
}

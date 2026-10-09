//! A call the client stops waiting for is cancelled, the way its transport
//! cancels: under MCP 2026-07-28 over Streamable HTTP by closing the call's
//! stream, with no `notifications/cancelled`; to a legacy peer by that
//! notification. Either on a timeout or when the caller drops the call.
#![cfg(all(feature = "http-server-volga", feature = "http-client"))]

mod common;

use neva::client::Client;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[cfg(not(feature = "legacy-spec"))]
mod streamable_http {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;

    /// Waits up to `limit` for `flag`.
    async fn raised(flag: &AtomicBool, limit: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + limit;
        while !flag.load(Ordering::SeqCst) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        flag.load(Ordering::SeqCst)
    }
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    /// A 2026-07-28 peer that answers `server/discover`, holds every
    /// `tools/call` open without answering, and notes what it was sent and
    /// whether the held call's connection was closed.
    struct Holding {
        addr: String,
        opened: Arc<AtomicBool>,
        hung_up: Arc<AtomicBool>,
        methods: Arc<Mutex<Vec<String>>>,
    }

    impl Holding {
        async fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let server = Self {
                addr: listener.local_addr().unwrap().to_string(),
                opened: Arc::default(),
                hung_up: Arc::default(),
                methods: Arc::default(),
            };
            let (opened, hung_up, methods) = (
                server.opened.clone(),
                server.hung_up.clone(),
                server.methods.clone(),
            );
            tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    tokio::spawn(serve(
                        stream,
                        opened.clone(),
                        hung_up.clone(),
                        methods.clone(),
                    ));
                }
            });
            server
        }

        fn sent(&self, method: &str) -> bool {
            self.methods.lock().unwrap().iter().any(|m| m == method)
        }
    }

    async fn serve(
        mut stream: TcpStream,
        opened: Arc<AtomicBool>,
        hung_up: Arc<AtomicBool>,
        methods: Arc<Mutex<Vec<String>>>,
    ) {
        while let Some(body) = read_body(&mut stream).await {
            let msg: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let method = msg["method"].as_str().unwrap_or_default().to_owned();
            methods.lock().unwrap().push(method.clone());

            if method == neva::commands::DISCOVER {
                let reply = serde_json::json!({
                    "jsonrpc": "2.0", "id": msg["id"],
                    "result": {
                        "supportedVersions": ["2026-07-28"],
                        "capabilities": { "tools": {} }
                    }
                });
                write_json(&mut stream, &reply.to_string()).await;
                continue;
            }
            if method != "tools/call" {
                let _ = stream
                    .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
                    .await;
                continue;
            }

            // Held: the only way this connection ends is the client closing it.
            opened.store(true, Ordering::SeqCst);
            let mut probe = [0u8; 1];
            if let Ok(Ok(0)) =
                tokio::time::timeout(Duration::from_secs(5), stream.read(&mut probe)).await
            {
                hung_up.store(true, Ordering::SeqCst);
            }
            return;
        }
    }

    async fn read_body(stream: &mut TcpStream) -> Option<String> {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 2048];
        let head_end = loop {
            let n = stream.read(&mut chunk).await.ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break at + 4;
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase();
        let length = head
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(0);
        while buf.len() < head_end + length {
            let n = stream.read(&mut chunk).await.ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        Some(String::from_utf8_lossy(&buf[head_end..head_end + length]).into_owned())
    }

    async fn write_json(stream: &mut TcpStream, body: &str) {
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(reply.as_bytes()).await;
    }

    async fn connect(server: &Holding, timeout: Duration) -> Client {
        let mut client = Client::new()
            .with_options(|o| o.with_http(|h| h.bind(&server.addr)).with_timeout(timeout));
        client.connect().await.expect("connect");
        client
    }

    /// MCP asks a sender to cancel a request it stopped waiting for; over
    /// Streamable HTTP that is closing the request's stream, and no
    /// notification is expected.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_call_that_times_out_closes_its_stream() {
        let server = Holding::start().await;
        let client = connect(&server, Duration::from_secs(1)).await;

        let err = client
            .tools()
            .call("slow", ())
            .await
            .expect_err("the call times out");
        assert!(err.to_string().contains("timed out"), "{err}");

        assert!(
            raised(&server.hung_up, Duration::from_secs(3)).await,
            "the held call's connection must be closed"
        );
        assert!(
            !server.sent("notifications/cancelled"),
            "no notification on Streamable HTTP"
        );
    }

    /// A caller that drops the call is not waiting either.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_dropped_call_closes_its_stream() {
        let server = Holding::start().await;
        let client = Arc::new(connect(&server, Duration::from_secs(30)).await);

        let caller = client.clone();
        let call = tokio::spawn(async move { caller.tools().call("slow", ()).await });
        assert!(
            raised(&server.opened, Duration::from_secs(3)).await,
            "the call reaches the server"
        );
        call.abort();

        assert!(
            raised(&server.hung_up, Duration::from_secs(3)).await,
            "the held call's connection must be closed"
        );
        assert!(!server.sent("notifications/cancelled"));
    }
}

/// Against neva's own server, in both profiles: the server stops a call the
/// client gave up on, whether it was told by the stream closing (2026-07-28)
/// or by `notifications/cancelled` (legacy).
mod against_neva {
    use super::*;
    use neva::App;
    use neva::types::{CallToolRequestParams, MessageEnvelope, Request, RequestId};
    use std::sync::atomic::AtomicUsize;

    /// Counts the tool handlers that were dropped, as a cancelled one is.
    struct CountOnDrop(Arc<AtomicUsize>);

    impl Drop for CountOnDrop {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Serves `slow`, which counts its handlers starting and being dropped,
    /// behind a middleware that holds each call for `hold`.
    async fn serve(
        hold: Duration,
    ) -> (
        String,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        let addr = format!("127.0.0.1:{}", common::free_port());
        let started = Arc::new(AtomicUsize::new(0));
        let stopped = Arc::new(AtomicUsize::new(0));

        let (on_start, on_stop) = (started.clone(), stopped.clone());
        let mut app = App::new()
            .without_greeting()
            .with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")));
        app.map_tool("slow", move || {
            let (on_start, on_stop) = (on_start.clone(), on_stop.clone());
            async move {
                on_start.fetch_add(1, Ordering::SeqCst);
                let _stopped = CountOnDrop(on_stop);
                tokio::time::sleep(Duration::from_secs(30)).await;
                "late"
            }
        });
        let app = app.wrap_tools(move |ctx, next| async move {
            tokio::time::sleep(hold).await;
            next(ctx).await
        });
        let server = tokio::spawn(async move { app.run().await });
        common::serving(&addr, &server).await;

        (addr, started, stopped, server)
    }

    async fn connect(addr: &str, timeout: Duration) -> Client {
        let mut client = Client::new().with_options(|o| {
            o.with_http(|h| h.bind(addr).with_endpoint("/mcp"))
                .with_timeout(timeout)
        });
        client.connect().await.expect("connect");
        client
    }

    /// Whether, after `wait`, every handler that started was also stopped.
    ///
    /// A cancel may land before a handler starts, so none starting is a pass
    /// as well; what may not happen is one left running. The wait is fixed
    /// rather than polled: no handler may be counted as stopped before it had
    /// its chance to start.
    async fn none_left_running(
        started: &AtomicUsize,
        stopped: &AtomicUsize,
        wait: Duration,
    ) -> (usize, usize) {
        tokio::time::sleep(wait).await;
        (
            started.load(Ordering::SeqCst),
            stopped.load(Ordering::SeqCst),
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_call_that_times_out_is_cancelled_on_the_server() {
        let (addr, started, stopped, server) = serve(Duration::ZERO).await;
        let client = connect(&addr, Duration::from_secs(1)).await;

        let err = client
            .tools()
            .call("slow", ())
            .await
            .expect_err("the call times out");
        assert!(err.to_string().contains("timed out"), "{err}");

        let (started, stopped) =
            none_left_running(&started, &stopped, Duration::from_millis(1500)).await;
        assert_eq!(started, stopped, "the server must stop the call");

        server.abort();
    }

    /// A cancel that reaches the server while its request is still on the way
    /// to the handler -- held up here by a middleware for a second -- stops it
    /// all the same:
    /// the server tracks a request as it reads it, not once the handler starts.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_call_dropped_before_its_handler_starts_never_runs() {
        let (addr, started, _stopped, server) = serve(Duration::from_secs(1)).await;
        let client = Arc::new(connect(&addr, Duration::from_secs(30)).await);

        let caller = client.clone();
        let call = tokio::spawn(async move { caller.tools().call("slow", ()).await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        call.abort();

        // Well past the middleware's hold.
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(
            started.load(Ordering::SeqCst),
            0,
            "a call cancelled on its way must not reach its handler"
        );

        server.abort();
    }

    fn slow_call(id: i64) -> MessageEnvelope {
        MessageEnvelope::Request(Request::new(
            Some(RequestId::Number(id)),
            "tools/call",
            Some(CallToolRequestParams::new("slow")),
        ))
    }

    /// The requests of a batch the caller gave up on are each cancelled.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_abandoned_batch_is_cancelled_on_the_server() {
        let (addr, started, stopped, server) = serve(Duration::ZERO).await;
        let client = connect(&addr, Duration::from_secs(1)).await;

        let call = slow_call;
        let err = client
            .call_batch(vec![call(1), call(2)])
            .await
            .expect_err("the batch times out");
        assert!(err.to_string().contains("timed out"), "{err}");

        let (started, stopped) =
            none_left_running(&started, &stopped, Duration::from_millis(1500)).await;
        assert_eq!(started, stopped, "the server must stop both calls");

        server.abort();
    }

    /// A batch given up on while a middleware holds its requests on the way to
    /// their handlers: the server has them tracked from the moment it read the
    /// batch, so none starts.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_batch_dropped_before_its_handlers_start_never_runs() {
        let (addr, started, _stopped, server) = serve(Duration::from_secs(1)).await;
        let client = Arc::new(connect(&addr, Duration::from_secs(30)).await);

        let caller = client.clone();
        let call =
            tokio::spawn(async move { caller.call_batch(vec![slow_call(1), slow_call(2)]).await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        call.abort();

        // Well past the middleware's hold.
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(
            started.load(Ordering::SeqCst),
            0,
            "a batch cancelled on its way must not reach its handlers"
        );

        server.abort();
    }
}

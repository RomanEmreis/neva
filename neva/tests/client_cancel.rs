//! A call the client stops waiting for is cancelled, the way its transport
//! cancels: under MCP 2026-07-28 over Streamable HTTP by closing the call's
//! stream, with no `notifications/cancelled`; to a legacy peer by that
//! notification. Either on a timeout or when the caller drops the call.
#![cfg(all(feature = "http-server-volga", feature = "http-client"))]

use neva::client::Client;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Waits up to `limit` for `flag`.
async fn raised(flag: &AtomicBool, limit: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    while !flag.load(Ordering::SeqCst) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    flag.load(Ordering::SeqCst)
}

#[cfg(not(feature = "legacy-spec"))]
mod streamable_http {
    use super::*;
    use std::sync::Mutex;
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
        let client = connect(&server, Duration::from_millis(300)).await;

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

#[cfg(feature = "legacy-spec")]
mod legacy {
    use super::*;
    use neva::App;

    /// Raises its flag when the tool's handler is dropped, as a cancelled one
    /// is.
    struct RaiseOnDrop(Arc<AtomicBool>);

    impl Drop for RaiseOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    fn pick_free_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    }

    /// To a legacy peer the client sends `notifications/cancelled`, on the
    /// session the call was made on, and the server stops the call.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_call_that_times_out_is_cancelled_on_the_server() {
        let addr = format!("127.0.0.1:{}", pick_free_port());
        let stopped = Arc::new(AtomicBool::new(false));

        let flag = stopped.clone();
        let mut app = App::new()
            .without_greeting()
            .with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")));
        app.map_tool("slow", move || {
            let flag = flag.clone();
            async move {
                let _stopped = RaiseOnDrop(flag);
                tokio::time::sleep(Duration::from_secs(30)).await;
                "late"
            }
        });
        let server = tokio::spawn(async move { app.run().await });
        tokio::time::sleep(Duration::from_millis(300)).await;

        let mut client = Client::new().with_options(|o| {
            o.with_http(|h| h.bind(&addr).with_endpoint("/mcp"))
                .with_timeout(Duration::from_millis(300))
        });
        client.connect().await.expect("connect");

        let err = client
            .tools()
            .call("slow", ())
            .await
            .expect_err("the call times out");
        assert!(err.to_string().contains("timed out"), "{err}");

        assert!(
            raised(&stopped, Duration::from_secs(3)).await,
            "the server must stop the call"
        );

        client.disconnect().await.ok();
        server.abort();
    }
}

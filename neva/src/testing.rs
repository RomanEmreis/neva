//! Drives an [`App`] from a test, with no child process and no port.
//!
//! [`TestClient`] runs the server on a task of the current runtime and
//! connects a [`Client`] to it over an in-memory pipe. The pipe carries the
//! same line-delimited JSON the stdio transport does, through the same reader
//! and writer tasks, so a request takes the dispatch path it takes on the wire
//! and a type that does not survive serialization fails here as it would
//! there.
//!
//! ```
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() {
//! use neva::{App, testing::TestClient};
//!
//! let mut app = App::new();
//! app.map_tool("hello", |name: String| async move { format!("Hello, {name}!") })
//!     .with_arg_names(["name"]);
//!
//! let client = TestClient::connect(app).await.expect("connect");
//! let result = client
//!     .tools()
//!     .call("hello", [("name", "world")])
//!     .await
//!     .expect("tool call");
//! assert_eq!(result.content[0].as_text().unwrap().text, "Hello, world!");
//!
//! client.shutdown().await.expect("shutdown");
//! # }
//! ```
//!
//! # A client that needs handlers
//!
//! Anything the server asks of the client -- elicitation, sampling, roots,
//! which is what MRTR is made of -- is answered by a handler on the
//! [`Client`]. Configure one and hand it to [`TestClient::connect_with`]; the
//! transport of the client you pass is replaced.
//!
//! ```
//! # #[cfg(not(feature = "legacy-spec"))]
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() {
//! use neva::{App, Client, Context, error::Error, testing::TestClient};
//! use neva::types::elicitation::{ElicitRequestParams, ElicitResult};
//!
//! let mut app = App::new();
//! app.map_tool("greet", |ctx: Context| async move {
//!     let ask: ElicitRequestParams = ElicitRequestParams::form("Your name?")
//!         .with_required("name", "string")
//!         .into();
//!     let answer = ctx.elicit("name", ask).await?;
//!     let name = answer
//!         .content
//!         .and_then(|c| c.get("name").and_then(|v| v.as_str().map(str::to_owned)))
//!         .unwrap_or_default();
//!     Ok::<_, Error>(format!("hello {name}"))
//! });
//!
//! let mut client = Client::new();
//! client.map_elicitation(|_| async {
//!     ElicitResult::accept().with_content(serde_json::json!({ "name": "octocat" }))
//! });
//!
//! let client = TestClient::connect_with(app, client).await.expect("connect");
//! let result = client.tools().call("greet", ()).await.expect("tool call");
//! assert_eq!(result.content[0].as_text().unwrap().text, "hello octocat");
//! client.shutdown().await.expect("shutdown");
//! # }
//! # #[cfg(feature = "legacy-spec")]
//! # fn main() {}
//! ```
//!
//! # What to know
//!
//! * **Stopping.** [`TestClient::shutdown`] stops the server the way
//!   [`App::with_shutdown`] does -- live `subscriptions/listen` streams are
//!   answered first -- waits for it, and disconnects the client. A
//!   `TestClient` dropped without it asks the server to stop and does not wait.
//! * **Panics.** A tool handler that panics is answered with an
//!   [`InternalError`](crate::error::ErrorCode::InternalError) response and
//!   the call returns `Err`; the test is not failed for you, so assert on the
//!   result. A panic outside the handler's own future -- in middleware
//!   wrapped around it, say -- is not caught: it ends the task serving that
//!   request, and the call is never answered. Startup panics are
//!   different: if the server stops before the client has connected,
//!   [`connect`](TestClient::connect) re-raises its panic.
//! * **Greeting.** The server's startup banner is turned off.
//! * **`#[tool]` and friends.** With the `macros` feature every [`App`] in a
//!   process picks up *all* the `#[tool]`, `#[prompt]` and `#[resource]`
//!   functions linked into the test binary, not only the ones the test names.
//!   Tests that need different sets belong in different test files.
//! * **Not carried.** Request-scoped `notifications/message` and
//!   `notifications/progress` come from the `tracing` layer, which on stdio
//!   writes them to the process's own stdout rather than into the transport,
//!   so they do not reach a [`TestClient`]. Subscriptions
//!   (`subscriptions/listen`) do.
//! * **Not tested.** Transport behaviour that is not stdio's: HTTP headers,
//!   sessions, the dual-mode `initialize` fallback over HTTP, OAuth.

use crate::{App, Client, app::ShutdownHandle, error::Error, error::ErrorCode};
use std::{panic::resume_unwind, time::Duration};
use tokio::{io::duplex, task::JoinHandle};

/// How much one end of the pipe buffers before a write waits for a read.
const PIPE_CAPACITY: usize = 64 * 1024;

/// How long [`TestClient::shutdown`] waits for the server task to end.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// A [`Client`] connected to an [`App`] running on a task of the current
/// runtime, over an in-memory pipe.
///
/// Dereferences to the [`Client`], so it is used as one. See the
/// [module documentation](self) for what it does and does not cover.
#[derive(Debug)]
pub struct TestClient {
    client: Client,
    server: Server,
}

/// The running server: what stops it, and the task to wait for.
#[derive(Debug)]
struct Server {
    shutdown: ShutdownHandle,
    task: Option<JoinHandle<()>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown.shutdown();
    }
}

impl TestClient {
    /// Starts `app` and connects a default [`Client`] to it.
    ///
    /// # Errors
    /// Returns the [`Error`] of a failed handshake.
    ///
    /// # Panics
    /// Re-raises the panic of a server that panicked before the client
    /// connected.
    pub async fn connect(app: App) -> Result<Self, Error> {
        Self::connect_with(app, Client::new()).await
    }

    /// Starts `app` and connects `client` to it, replacing the transport the
    /// client was configured with.
    ///
    /// Use it for a client that has handlers (`map_elicitation`, roots, ...)
    /// or options (`with_timeout`, ...) of its own.
    ///
    /// # Errors
    /// Returns the [`Error`] of a failed handshake.
    ///
    /// # Panics
    /// Re-raises the panic of a server that panicked before the client
    /// connected.
    pub async fn connect_with(app: App, client: Client) -> Result<Self, Error> {
        let (server_end, client_end) = duplex(PIPE_CAPACITY);

        let (app, shutdown) = app
            .without_greeting()
            .with_options(|opt| opt.with_in_memory(server_end))
            .with_shutdown();
        let mut client = client.with_options(|opt| opt.with_in_memory(client_end));

        let mut task = tokio::spawn(app.run());

        let connected = tokio::select! {
            connected = client.connect() => connected,
            ended = &mut task => {
                return Err(match ended {
                    Err(err) if err.is_panic() => resume_unwind(err.into_panic()),
                    _ => Error::new(
                        ErrorCode::InternalError,
                        "the server stopped before the client connected",
                    ),
                });
            }
        };

        let server = Server {
            shutdown,
            task: Some(task),
        };
        connected.map(|()| Self { client, server })
    }

    /// A handle that stops the server without disconnecting the client, for a
    /// test of what the client sees when the server goes away.
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        self.server.shutdown.clone()
    }

    /// Stops the server, waits for it, and disconnects the client.
    ///
    /// # Errors
    /// Returns an [`Error`] when the server does not stop within ten seconds.
    ///
    /// # Panics
    /// Re-raises the panic of a server task that panicked.
    pub async fn shutdown(self) -> Result<(), Error> {
        let Self { client, mut server } = self;

        server.shutdown.shutdown();
        if let Some(task) = server.task.take() {
            match tokio::time::timeout(STOP_TIMEOUT, task).await {
                Err(_) => {
                    return Err(Error::new(
                        ErrorCode::Timeout,
                        "the server did not stop within ten seconds of the shutdown request",
                    ));
                }
                Ok(Err(err)) if err.is_panic() => resume_unwind(err.into_panic()),
                Ok(_) => {}
            }
        }

        client.disconnect().await
    }
}

impl std::ops::Deref for TestClient {
    type Target = Client;

    #[inline]
    fn deref(&self) -> &Client {
        &self.client
    }
}

impl std::ops::DerefMut for TestClient {
    #[inline]
    fn deref_mut(&mut self) -> &mut Client {
        &mut self.client
    }
}

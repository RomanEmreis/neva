//! Building a client, and getting it talking to a server.
//!
//! The handshake is where the dual-mode client earns its name. A 2026-07-28
//! build opens with `server/discover`; if the server answers in a way that says
//! it does not speak this generation -- and only then, see
//! [`is_fallback_trigger`] -- the client flips to the legacy `initialize`
//! handshake once and stays there. A network failure is not such an answer, so
//! it is not a trigger: falling back would only mask the outage.

use super::*;

impl Client {
    /// Initializes a new client app
    pub fn new() -> Self {
        Self {
            options: McpOptions::default(),
            server_capabilities: None,
            server_info: None,
            cancellation_token: None,
            handler: None,
        }
    }

    /// Configure MCP client options
    pub fn with_options<F>(mut self, config: F) -> Self
    where
        F: FnOnce(McpOptions) -> McpOptions,
    {
        self.options = config(self.options);
        self
    }

    /// Adds a new Root
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// # use neva::error::Error;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Error> {
    /// let mut client = Client::new();
    /// client.add_root("file:///home/user/projects/my_project", "My Project");
    /// # client.disconnect().await
    /// # }
    /// ```
    #[deprecated(
        note = "Roots are deprecated in MCP 2026-07-28: the capability-driven `roots/list` request is gone and the ability is re-homed onto MRTR -- see `Context::list_roots`. Under MCP 2026-07-28 this configures what the client answers MRTR `roots/list` input requests with."
    )]
    #[allow(deprecated)]
    pub fn add_root(&mut self, uri: impl Into<Uri>, name: impl Into<String>) -> &mut Self {
        self.options.add_root(Root::new(uri, name));
        self.publish_roots_changed();
        self
    }

    /// Adds multiple new Roots.
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// # use neva::error::Error;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Error> {
    /// let mut client = Client::new();
    /// client.add_roots([
    ///     ("file:///home/user/projects/my_project", "My Project"),
    ///     ("file:///home/user/projects/another_project", "My Another Project")
    /// ]);
    /// # client.disconnect().await
    /// # }
    /// ```
    #[deprecated(
        note = "Roots are deprecated in MCP 2026-07-28: the capability-driven `roots/list` request is gone and the ability is re-homed onto MRTR -- see `Context::list_roots`. Under MCP 2026-07-28 this configures what the client answers MRTR `roots/list` input requests with."
    )]
    #[allow(deprecated)]
    pub fn add_roots<T, I>(&mut self, roots: I) -> &mut Self
    where
        T: Into<Root>,
        I: IntoIterator<Item = T>,
    {
        self.options.add_roots(roots);
        self.publish_roots_changed();
        self
    }

    /// Sends the "notifications/roots/list_changed" notification to the server
    #[deprecated(
        note = "Roots are deprecated in MCP 2026-07-28: the capability-driven `roots/list` request is gone and the ability is re-homed onto MRTR -- see `Context::list_roots`. Under MCP 2026-07-28 this configures what the client answers MRTR `roots/list` input requests with."
    )]
    pub fn publish_roots_changed(&mut self) {
        if let Some(handler) = self.handler.as_mut() {
            let roots = self.options.roots();
            handler.notify_roots_changed(roots);
        }
    }

    /// Registers a handler that will be running when a "sampling/createMessage" request is received
    #[deprecated(
        note = "Sampling is deprecated in MCP 2026-07-28: the capability-driven `sampling/createMessage` request is gone and the ability is re-homed onto MRTR -- see `Context::sample`. Under MCP 2026-07-28 this handler fulfils MRTR `sampling/createMessage` input requests."
    )]
    pub fn map_sampling<F, R>(&mut self, handler: F) -> &mut Self
    where
        F: Fn(CreateMessageRequestParams) -> R + Clone + Send + Sync + 'static,
        R: Future + Send,
        R::Output: Into<CreateMessageResult>,
    {
        let handler: SamplingHandler = make_handler(handler);
        self.options.add_sampling_handler(handler);
        self
    }

    /// Registers a handler that will be running when an "elicitation/create" request is received
    pub fn map_elicitation<F, R>(&mut self, handler: F) -> &mut Self
    where
        F: Fn(ElicitRequestParams) -> R + Clone + Send + Sync + 'static,
        R: Future + Send,
        R::Output: Into<ElicitResult>,
    {
        let handler: ElicitationHandler = make_handler(handler);
        self.options.add_elicitation_handler(handler);
        self
    }

    /// Connects the MCP client to the MCP server
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     // call tools, read resources, etc.
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn connect(&mut self) -> Result<(), Error> {
        #[cfg(feature = "macros")]
        self.register_methods();

        let mut transport = self.options.transport();
        // A client has no shutdown drain of its own to join -- see
        // `TransportHandle::detached` -- so only the cancellation half is kept.
        let token = transport.start()?.token;

        #[cfg(feature = "tracing")]
        self.register_tracing_notification_handlers();

        self.cancellation_token = Some(token.clone());
        self.handler = Some(RequestHandler::new(transport, &self.options, token));

        self.wait_for_shutdown_signal();
        self.init().await
    }

    /// Disconnects the MCP client from the MCP server
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///
    ///     client.connect().await?;
    ///
    ///     // call tools, read resources, etc.
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn disconnect(mut self) -> Result<(), Error> {
        // Closing the transport is the whole goodbye. This used to send a
        // param-less `notifications/cancelled` first, which was wrong twice
        // over: that notification cancels one named in-flight request and its
        // `params.requestId` is required, so without params it fails the spec's
        // own schema -- and no server has anything to act on either, neva's
        // included, which drops it for want of a request id. Under
        // MCP 2026-07-28 there is nothing to send in any case: the revision
        // defines no client-to-server notification on Streamable HTTP, where
        // closing the stream *is* the cancellation.
        //
        // It only reached the wire when the transport task picked it up before
        // the cancellation below landed, which is why it read as a flake rather
        // than a bug.
        if let Some(token) = self.cancellation_token.take() {
            token.cancel();
        }

        // Let the transport task observe the cancellation and drain before the
        // runtime goes away under it.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        Ok(())
    }

    /// The protocol version this client expects from the connected peer.
    ///
    /// Under MCP 2026-07-28 the 2026-07-28 expectation is pinned to
    /// [`crate::LATEST_PROTOCOL_VERSION`]: a `with_mcp_version` override only
    /// selects which legacy version the dual-mode fallback negotiates --
    /// it must never make `server/discover` reject a valid 2026-07-28 server.
    ///
    /// On the legacy profile the offered version is a proposal the server may
    /// answer with another supported one, so nothing downstream reads it back:
    /// the sole consumer there is the (tracing-gated) negotiation log.
    #[cfg_attr(
        all(feature = "legacy-spec", not(feature = "tracing")),
        allow(dead_code)
    )]
    pub(super) fn expected_protocol_ver(&self) -> &'static str {
        #[cfg(not(feature = "legacy-spec"))]
        {
            if self.is_legacy_peer() {
                self.options.legacy_protocol_ver()
            } else {
                crate::LATEST_PROTOCOL_VERSION
            }
        }
        #[cfg(feature = "legacy-spec")]
        {
            self.options.protocol_ver()
        }
    }

    /// Validates the protocol version the server answered the handshake with,
    /// cancelling the transport when this client cannot speak it.
    ///
    /// The version the client offers is a proposal, not a demand: a server that
    /// does not speak it answers with one it does, and the handshake succeeds if
    /// the client speaks that. Only a version outside
    /// [`PROTOCOL_VERSIONS`](crate::PROTOCOL_VERSIONS) ends the connection --
    /// insisting on the offered version instead would refuse every server a
    /// notch older than the client, which is the case the negotiation exists
    /// for.
    pub(super) fn validate_server_version(&mut self, server_ver: &str) -> Result<(), Error> {
        if !crate::PROTOCOL_VERSIONS.contains(&server_ver) {
            self.cancel_transport();
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                format!("Unsupported server protocol version: {server_ver}"),
            ));
        }

        #[cfg(feature = "tracing")]
        if server_ver != self.expected_protocol_ver() {
            tracing::info!(
                logger = "neva",
                "Server answered with protocol version {server_ver}, not the offered {}",
                self.expected_protocol_ver()
            );
        }
        Ok(())
    }

    /// Sends `initialize` request to an MCP server (legacy handshake).
    #[cfg(feature = "legacy-spec")]
    pub async fn init(&mut self) -> Result<(), Error> {
        self.legacy_init().await
    }

    /// The `initialize`/`initialized` handshake: the only handshake for
    /// the legacy build, the dual-mode fallback for the 2026-07-28 build.
    pub(super) async fn legacy_init(&mut self) -> Result<(), Error> {
        #[cfg(feature = "legacy-spec")]
        let protocol_ver = self.options.protocol_ver().to_string();
        // The fallback negotiates the newest legacy version -- offering
        // the 2026-07-28 version to a server that just rejected `server/discover`
        // would only get refused again.
        #[cfg(not(feature = "legacy-spec"))]
        let protocol_ver = self.options.legacy_protocol_ver().to_string();

        let params = InitializeRequestParams {
            protocol_ver,
            client_info: Some(self.options.implementation.clone()),
            capabilities: Some(ClientCapabilities {
                roots: self.options.roots_capability(),
                sampling: self.options.sampling_capability(),
                elicitation: self.options.elicitation_capability(),
                #[cfg(feature = "tasks")]
                tasks: self.options.tasks_capability(),
                extensions: self.options.extensions(),
                experimental: None,
            }),
        };

        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            crate::commands::INIT,
            Some(params),
        );

        let resp = self.send_request(req).await?;

        let init_result = resp.into_result::<InitializeResult>()?;

        self.validate_server_version(init_result.protocol_ver.as_str())?;

        self.server_capabilities = Some(init_result.capabilities);
        self.server_info = Some(init_result.server_info);

        self.send_notification(crate::types::notification::commands::INITIALIZED, None)
            .await
    }

    /// Discovers server capabilities via `server/discover` (MCP 2026-07-28).
    ///
    /// Replaces the `initialize`/`initialized` handshake. No `initialized`
    /// notification is sent -- the transport is stateless.
    #[cfg(not(feature = "legacy-spec"))]
    pub async fn discover(&mut self) -> Result<(), Error> {
        let resp = self.send_request(Self::discover_request()).await?;
        let result = resp.into_result::<crate::types::DiscoverResult>()?;
        self.apply_discover(result)
    }

    /// Builds the `server/discover` request.
    #[cfg(not(feature = "legacy-spec"))]
    pub(super) fn discover_request() -> Request {
        Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            crate::commands::DISCOVER,
            Some(crate::types::DiscoverRequestParams::default()),
        )
    }

    /// Applies a successful `server/discover` result: validates the
    /// reported protocol version and stores the capabilities.
    #[cfg(not(feature = "legacy-spec"))]
    pub(super) fn apply_discover(
        &mut self,
        result: crate::types::DiscoverResult,
    ) -> Result<(), Error> {
        // Discovery advertises a *set*; the handshake succeeds when the version
        // this client speaks is among them.
        let expected = self.expected_protocol_ver();
        if !result.supported_versions.iter().any(|v| v == expected) {
            // `connect` has already started the transport; leaving it running
            // would park background HTTP/SSE tasks behind a client that never
            // completed its handshake.
            self.cancel_transport();
            return Err(Error::new(
                ErrorCode::UnsupportedProtocolVersion,
                format!(
                    "Server supports {:?} but the client speaks {expected}",
                    result.supported_versions
                ),
            ));
        }
        self.server_capabilities = Some(result.capabilities);
        // `serverInfo` left `DiscoverResult` in the final spec: servers now
        // report themselves in every result's `_meta`, so it is picked up from
        // there instead.
        Ok(())
    }

    /// The dual-mode handshake (issue #84): tries `server/discover`
    /// first and, when the server clearly doesn't speak the 2026-07-28 protocol,
    /// falls back to the legacy `initialize` handshake and marks the
    /// peer as legacy -- subsequent traffic uses legacy semantics
    /// (session header, SSE stream, no MRTR, no 2026-07-28 routing headers).
    ///
    /// Only **wire-phase** failures classify for the fallback: transport
    /// errors and the server's JSON-RPC *error* reply. Once the server
    /// answers `server/discover` successfully, the peer has committed to
    /// the 2026-07-28 protocol -- later local failures (a malformed result, an
    /// unsupported/mismatched `protocolVersion`) surface as real errors
    /// instead of a misleading fallback attempt on a transport that
    /// version validation may already have cancelled.
    #[cfg(not(feature = "legacy-spec"))]
    pub async fn init(&mut self) -> Result<(), Error> {
        let resp = match self.send_request(Self::discover_request()).await {
            Ok(resp) => resp,
            Err(err) if is_fallback_trigger(&err) => return self.fallback_init(&err).await,
            Err(err) => return Err(err),
        };

        let is_error_reply = matches!(resp, Response::Err(_));
        let result = match resp.into_result::<crate::types::DiscoverResult>() {
            Ok(result) => result,
            Err(err) if is_error_reply && is_fallback_trigger(&err) => {
                return self.fallback_init(&err).await;
            }
            Err(err) => return Err(err),
        };

        self.apply_discover(result)
    }

    /// Runs the legacy half of the dual-mode handshake after a
    /// classified `server/discover` rejection.
    #[cfg(not(feature = "legacy-spec"))]
    pub(super) async fn fallback_init(&mut self, _err: &Error) -> Result<(), Error> {
        #[cfg(feature = "tracing")]
        tracing::info!(
            logger = "neva",
            "`server/discover` rejected ({_err}); falling back to `initialize`"
        );
        self.options.peer_mode.set_legacy();
        self.legacy_init().await
    }

    /// Whether the peer negotiated the legacy protocol through the
    /// dual-mode fallback.
    #[cfg(not(feature = "legacy-spec"))]
    pub(super) fn is_legacy_peer(&self) -> bool {
        self.options.peer_mode.is_legacy()
    }

    /// Sends a ping to the MCP server
    ///
    /// Removed in MCP 2026-07-28; available only under `legacy-spec`.
    #[cfg(feature = "legacy-spec")]
    pub async fn ping(&mut self) -> Result<Response, Error> {
        self.command::<()>(crate::commands::PING, None).await
    }
}

/// Whether a `server/discover` failure means "this server doesn't speak
/// the 2026-07-28 protocol" -- the dual-mode fallback triggers only then.
///
/// * `MethodNotFound` -- a legacy server rejecting the unknown method
///   (neva's own legacy build answers exactly this);
/// * `InvalidRequest` -- strict servers rejecting the 2026-07-28 request shape;
/// * `ParseError` -- a non-JSON-RPC reply (an HTTP 4xx page) or an error
///   code outside neva's `ErrorCode` set (e.g. the TS SDK's `-32000`
///   "server not initialized"), both of which surface as parse failures.
///
/// Network-level failures (`Timeout`, `InternalError`/"Connection
/// closed") are *not* triggers: the server never answered, so falling
/// back would only mask the outage.
#[cfg(not(feature = "legacy-spec"))]
fn is_fallback_trigger(err: &Error) -> bool {
    matches!(
        err.code,
        ErrorCode::MethodNotFound | ErrorCode::InvalidRequest | ErrorCode::ParseError
    )
}

#[cfg(all(test, not(feature = "legacy-spec")))]
mod fallback_trigger_tests {
    use super::*;

    fn err(code: ErrorCode) -> Error {
        Error::new(code, "test")
    }

    #[test]
    fn protocol_level_rejections_trigger_the_fallback() {
        assert!(is_fallback_trigger(&err(ErrorCode::MethodNotFound)));
        assert!(is_fallback_trigger(&err(ErrorCode::InvalidRequest)));
        // Non-JSON-RPC replies (HTTP 4xx pages) and unknown error codes
        // (e.g. the TS SDK's -32000) surface as parse failures.
        assert!(is_fallback_trigger(&err(ErrorCode::ParseError)));
    }

    #[test]
    fn transport_failures_do_not_trigger_the_fallback() {
        assert!(!is_fallback_trigger(&err(ErrorCode::Timeout)));
        assert!(!is_fallback_trigger(&err(ErrorCode::InternalError)));
        assert!(!is_fallback_trigger(&err(ErrorCode::InvalidParams)));
    }
}

/// The dual-mode "Done when" (issue #84): a 2026-07-28 client completes calls
/// against a 2025-11-25 server via the `initialize` fallback. The legacy
/// server is a raw-HTTP mock because a legacy neva server cannot exist
/// in an 2026-07-28 build.
#[cfg(all(test, feature = "http-client", not(feature = "legacy-spec")))]
mod dual_mode_tests {
    use super::*;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    const LEGACY_SESSION_ID: &str = "6f2f0dc8-6a5e-4f6e-9c1a-2b7f9d3f1c11";

    /// Reads one HTTP/1.1 request; returns `(head, body)`.
    async fn read_request(stream: &mut TcpStream) -> Option<(String, String)> {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 2048];
        let header_end = loop {
            let n = stream.read(&mut tmp).await.ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&tmp[..n]);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
            if buf.len() > 65536 {
                return None;
            }
        };
        let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
        let content_length = head
            .lines()
            .find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|v| v.trim().parse::<usize>().ok())
            })
            .flatten()
            .unwrap_or(0);
        while buf.len() < header_end + content_length {
            let n = stream.read(&mut tmp).await.ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        let body =
            String::from_utf8_lossy(&buf[header_end..header_end + content_length]).to_string();
        Some((head, body))
    }

    async fn write_response(stream: &mut TcpStream, status: &str, extra_headers: &str, body: &str) {
        let resp = format!(
            "HTTP/1.1 {status}\r\n{extra_headers}Content-Length: {}\r\nConnection: keep-alive\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(resp.as_bytes()).await;
    }

    fn rpc_result(id: &serde_json::Value, result: serde_json::Value) -> String {
        serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
    }

    /// A minimal 2025-11-25 server: rejects `server/discover` with
    /// `MethodNotFound`, answers `initialize` with a session id, serves
    /// an SSE GET stream and an empty `tools/list`.
    /// How the mock answers `server/discover`: legacy servers reject it
    /// with a JSON-RPC `MethodNotFound` (neva's own legacy build) or a
    /// plain non-JSON-RPC 4xx page (framework routers, the TS SDK's
    /// "server not initialized" family); a future server answers
    /// *successfully* but with a `protocolVersion` this build does not
    /// support -- which must NOT trigger the fallback.
    /// A protected endpoint answering `401` with a non-JSON body must not
    /// trigger the fallback either -- that is an authentication failure,
    /// not evidence of a legacy peer.
    #[derive(Clone, Copy)]
    enum DiscoverReply {
        MethodNotFound,
        Html400,
        UnsupportedVersion,
        Unauthorized401,
        Unavailable503,
    }

    async fn serve_legacy(
        listener: TcpListener,
        log: Arc<Mutex<Vec<String>>>,
        reply: DiscoverReply,
    ) {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let log = log.clone();
            tokio::spawn(async move {
                loop {
                    let Some((head, body)) = read_request(&mut stream).await else {
                        return;
                    };
                    log.lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(format!("{head}{body}"));

                    if head.starts_with("GET") {
                        let _ = stream
                            .write_all(
                                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\n: hi\n\n",
                            )
                            .await;
                        // Hold the stream open like a real legacy server.
                        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                        return;
                    }

                    let msg: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                    let id = msg.get("id").cloned().unwrap_or(serde_json::Value::Null);
                    match msg
                        .get("method")
                        .and_then(|m| m.as_str())
                        .unwrap_or_default()
                    {
                        crate::commands::DISCOVER => match reply {
                            DiscoverReply::MethodNotFound => {
                                let body = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "error": { "code": -32601, "message": "Method not found" }
                                })
                                .to_string();
                                write_response(
                                    &mut stream,
                                    "200 OK",
                                    "Content-Type: application/json\r\n",
                                    &body,
                                )
                                .await;
                            }
                            DiscoverReply::Html400 => {
                                write_response(
                                    &mut stream,
                                    "400 Bad Request",
                                    "Content-Type: text/html\r\n",
                                    "<html><body>Bad Request</body></html>",
                                )
                                .await;
                            }
                            DiscoverReply::Unavailable503 => {
                                write_response(
                                    &mut stream,
                                    "503 Service Unavailable",
                                    "Content-Type: text/html\r\n",
                                    "<html><body>upstream down</body></html>",
                                )
                                .await;
                            }
                            DiscoverReply::Unauthorized401 => {
                                write_response(
                                    &mut stream,
                                    "401 Unauthorized",
                                    "Content-Type: text/html\r\nWWW-Authenticate: Bearer\r\n",
                                    "<html><body>Unauthorized</body></html>",
                                )
                                .await;
                            }
                            DiscoverReply::UnsupportedVersion => {
                                let body = rpc_result(
                                    &id,
                                    serde_json::json!({
                                        "supportedVersions": ["2099-01-01"],
                                        "capabilities": { "tools": {} }
                                    }),
                                );
                                write_response(
                                    &mut stream,
                                    "200 OK",
                                    "Content-Type: application/json\r\n",
                                    &body,
                                )
                                .await;
                            }
                        },
                        crate::commands::INIT => {
                            let body = rpc_result(
                                &id,
                                serde_json::json!({
                                    "protocolVersion": "2025-11-25",
                                    "capabilities": { "tools": {} },
                                    "serverInfo": { "name": "legacy-mock", "version": "1.0.0" }
                                }),
                            );
                            let headers = format!(
                                "Content-Type: application/json\r\nMcp-Session-Id: {LEGACY_SESSION_ID}\r\n"
                            );
                            write_response(&mut stream, "200 OK", &headers, &body).await;
                        }
                        crate::types::tool::commands::LIST => {
                            let body = rpc_result(&id, serde_json::json!({ "tools": [] }));
                            write_response(
                                &mut stream,
                                "200 OK",
                                "Content-Type: application/json\r\n",
                                &body,
                            )
                            .await;
                        }
                        // notifications (initialized / cancelled)
                        _ => write_response(&mut stream, "202 Accepted", "", "").await,
                    }
                }
            });
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn client_falls_back_to_initialize_against_legacy_server() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        tokio::spawn(serve_legacy(
            listener,
            log.clone(),
            DiscoverReply::MethodNotFound,
        ));

        let mut client = Client::new().with_options(|opt| {
            opt.with_http(|http| http.bind(addr.to_string()))
                .with_timeout(std::time::Duration::from_secs(5))
        });

        client
            .connect()
            .await
            .expect("fallback connect must succeed");
        assert!(client.is_legacy_peer(), "peer must be marked legacy");
        assert_eq!(
            client.server_info.as_ref().map(|i| i.name.as_str()),
            Some("legacy-mock")
        );

        let tools = client
            .list_tools(None)
            .await
            .expect("tools/list must work after the fallback");
        assert!(tools.tools.is_empty());

        let log = log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let discover = log
            .iter()
            .find(|r| r.contains("server/discover"))
            .expect("discover must be attempted first");
        assert!(
            discover
                .to_ascii_lowercase()
                .contains("mcp-protocol-version"),
            "the 2026-07-28 attempt carries the protocol-version header"
        );

        let init = log
            .iter()
            .find(|r| r.contains("\"method\":\"initialize\""))
            .expect("initialize must be sent after the rejection");
        assert!(
            init.contains("2025-11-25"),
            "the fallback negotiates the newest legacy version"
        );

        let list = log
            .iter()
            .find(|r| r.contains(crate::types::tool::commands::LIST))
            .expect("tools/list recorded");
        let list_lower = list.to_ascii_lowercase();
        assert!(
            !list_lower.contains("mcp-protocol-version"),
            "legacy peers must not receive the 2026-07-28 protocol-version header"
        );
        assert!(
            !list_lower.contains("mcp-method:"),
            "legacy peers must not receive 2026-07-28 routing headers"
        );
        assert!(
            list_lower.contains(&format!("mcp-session-id: {LEGACY_SESSION_ID}")),
            "the captured session id must ride every request"
        );
    }

    /// A legacy server that rejects `server/discover` with a plain
    /// non-JSON-RPC 4xx page (no JSON-RPC error at all) must also
    /// trigger the fallback: the transport completes the request with an
    /// id-bound `ParseError` response instead of a bare channel error.
    #[tokio::test(flavor = "multi_thread")]
    async fn client_falls_back_when_discover_gets_a_non_json_4xx() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        tokio::spawn(serve_legacy(listener, log.clone(), DiscoverReply::Html400));

        let mut client = Client::new().with_options(|opt| {
            opt.with_http(|http| http.bind(addr.to_string()))
                .with_timeout(std::time::Duration::from_secs(5))
        });

        client
            .connect()
            .await
            .expect("fallback connect must succeed");
        assert!(client.is_legacy_peer(), "peer must be marked legacy");

        let tools = client
            .list_tools(None)
            .await
            .expect("tools/list must work after the fallback");
        assert!(tools.tools.is_empty());
    }

    /// A server that answers `server/discover` *successfully* but with a
    /// protocol version this build does not support has committed to the
    /// 2026-07-28 path -- the client must surface the real version error, not
    /// mark the peer legacy and chase `initialize` on a cancelled
    /// transport.
    #[tokio::test(flavor = "multi_thread")]
    async fn successful_discover_with_unsupported_version_does_not_fall_back() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        tokio::spawn(serve_legacy(
            listener,
            log.clone(),
            DiscoverReply::UnsupportedVersion,
        ));

        let mut client = Client::new().with_options(|opt| {
            opt.with_http(|http| http.bind(addr.to_string()))
                .with_timeout(std::time::Duration::from_secs(5))
        });

        let err = client
            .connect()
            .await
            .expect_err("an unsupported 2026-07-28 version must fail the connect");
        assert!(
            err.to_string().contains("but the client speaks"),
            "the real version error must surface, got: {err}"
        );
        assert!(
            !client.is_legacy_peer(),
            "a successful discovery must never mark the peer legacy"
        );
        assert!(
            client.handler.is_none() && client.cancellation_token.is_none(),
            "a failed negotiation must not leave the transport running"
        );

        let log = log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            !log.iter().any(|r| r.contains("\"method\":\"initialize\"")),
            "no initialize fallback may be attempted after successful discovery"
        );
    }

    /// `notifications/roots/list_changed` is gone from MCP 2026-07-28, but a
    /// peer reached through the fallback negotiated `roots.listChanged` on the
    /// legacy protocol -- and would otherwise hold a stale root list forever.
    // Roots are deprecated under MCP 2026-07-28 -- which is exactly why the
    // fallback still owes the legacy peer this notification.
    #[allow(deprecated)]
    #[tokio::test(flavor = "multi_thread")]
    async fn roots_changes_are_pushed_to_a_fallback_peer() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        tokio::spawn(serve_legacy(
            listener,
            log.clone(),
            DiscoverReply::MethodNotFound,
        ));

        let mut client = Client::new().with_options(|opt| {
            opt.with_http(|http| http.bind(addr.to_string()))
                .with_roots(|roots| roots.with_list_changed())
                .with_timeout(std::time::Duration::from_secs(5))
        });

        client.connect().await.expect("fallback must connect");
        assert!(client.is_legacy_peer(), "the mock is a legacy server");

        client.add_root("file:///tmp/project", "Project");

        // The push is fire-and-forget through a spawned task.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let seen = log
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .any(|r| r.contains("notifications/roots/list_changed"));
            if seen {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "a legacy peer must be told its root list changed"
            );
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    /// A protected 2026-07-28 endpoint replying `401` with a non-JSON body must
    /// surface the authentication failure, not be mistaken for a legacy
    /// peer -- otherwise the client silently drops the 2026-07-28 headers and
    /// retries `initialize`, masking the real cause.
    #[tokio::test(flavor = "multi_thread")]
    async fn unauthorized_discover_does_not_fall_back() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        tokio::spawn(serve_legacy(
            listener,
            log.clone(),
            DiscoverReply::Unauthorized401,
        ));

        let mut client = Client::new().with_options(|opt| {
            opt.with_http(|http| http.bind(addr.to_string()))
                .with_timeout(std::time::Duration::from_secs(5))
        });

        let err = client
            .connect()
            .await
            .expect_err("an unauthenticated connect must fail");
        assert!(
            err.to_string().contains("401"),
            "the HTTP status must be carried through, got: {err}"
        );
        assert!(
            !client.is_legacy_peer(),
            "an auth failure must never mark the peer legacy"
        );

        let log = log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            !log.iter().any(|r| r.contains("\"method\":\"initialize\"")),
            "no initialize fallback may be attempted after an auth failure"
        );
    }

    /// An upstream outage (reverse proxy `503`, rate limit, gateway
    /// timeout) says nothing about the peer's protocol generation: the
    /// failure must surface instead of being read as "legacy" and retried
    /// as `initialize` into the very same outage.
    #[tokio::test(flavor = "multi_thread")]
    async fn upstream_failure_during_discover_does_not_fall_back() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        tokio::spawn(serve_legacy(
            listener,
            log.clone(),
            DiscoverReply::Unavailable503,
        ));

        let mut client = Client::new().with_options(|opt| {
            opt.with_http(|http| http.bind(addr.to_string()))
                .with_timeout(std::time::Duration::from_secs(5))
        });

        let err = client
            .connect()
            .await
            .expect_err("an upstream outage must fail the connect");
        assert!(
            err.to_string().contains("503"),
            "the upstream status must surface, got: {err}"
        );
        assert!(
            !client.is_legacy_peer(),
            "an upstream outage must never mark the peer legacy"
        );

        let log = log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            !log.iter().any(|r| r.contains("\"method\":\"initialize\"")),
            "no initialize fallback may be attempted on an upstream failure"
        );
    }

    /// A `with_mcp_version` legacy override must not leak into the 2026-07-28
    /// expectation: `server/discover` still expects the 2026-07-28 version, the
    /// override only selects the fallback's negotiated legacy version.
    #[test]
    fn legacy_version_override_keeps_the_latest_expectation() {
        let client = Client::new().with_options(|opt| opt.with_mcp_version("2025-06-18"));
        assert_eq!(
            client.expected_protocol_ver(),
            crate::LATEST_PROTOCOL_VERSION
        );

        client.options.peer_mode.set_legacy();
        assert_eq!(client.expected_protocol_ver(), "2025-06-18");
    }
}

/// The other half of the "Done when": against a 2026-07-28 server the client
/// keeps using `server/discover` (no fallback).
#[cfg(all(
    test,
    feature = "http-client",
    feature = "http-server-volga",
    not(feature = "legacy-spec")
))]
mod roundtrip_tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn client_discovers_a_2026_07_28_server() {
        let mut app = crate::App::new()
            .with_options(|opt| opt.with_http(|http| http.bind("127.0.0.1:39817")));
        app.map_tool("echo", |name: String| async move { name });
        tokio::spawn(app.run());

        // Wait until the server socket actually accepts connections --
        // a fixed sleep is not enough on loaded CI machines.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match tokio::net::TcpStream::connect("127.0.0.1:39817").await {
                Ok(_) => break,
                Err(_) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await
                }
                Err(err) => panic!("2026-07-28 server never became reachable: {err}"),
            }
        }

        let mut client = Client::new().with_options(|opt| {
            opt.with_http(|http| http.bind("127.0.0.1:39817"))
                .with_timeout(std::time::Duration::from_secs(5))
        });

        client.connect().await.expect("discover must succeed");
        assert!(!client.is_legacy_peer(), "2026-07-28 peers never fall back");

        let tools = client.list_tools(None).await.expect("tools/list");
        assert_eq!(tools.tools.len(), 1);
        assert_eq!(tools.tools[0].name, "echo");

        // `serverInfo` left `DiscoverResult`: it now rides in every result's
        // `_meta`, and the MRTR send path -- which every 2026-07-28 request
        // takes -- is what has to pick it up.
        assert!(
            client.server_info.is_some(),
            "the server identifies itself in every result's `_meta`"
        );
    }
}

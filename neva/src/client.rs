//! Utilities for the MCP client

use crate::error::{Error, ErrorCode};
use crate::shared;
use crate::shared::{BlockingCall, BlockingFn, BoxFuture, marker};
use crate::types::Root;
use crate::types::sampling::{CreateMessageRequestParams, CreateMessageResult, SamplingHandler};
use crate::types::{
    CallToolRequestParams, CallToolResponse, GetPromptResult, Implementation, ListPromptsResult,
    ListResourceTemplatesResult, ListResourcesResult, ListToolsRequestParams, ListToolsResult,
    MessageEnvelope, ReadResourceResult, Request, RequestId, Response, ServerCapabilities, Uri,
    cursor::Cursor,
    elicitation::{ElicitRequestParams, ElicitResult, ElicitationHandler},
    notification::Notification,
};
use crate::types::{ClientCapabilities, InitializeRequestParams, InitializeResult};
#[cfg(not(feature = "legacy-spec"))]
use crate::types::{SubscriptionFilter, SubscriptionsListenRequestParams};
use handler::RequestHandler;
use options::McpOptions;
use serde::Serialize;
use std::fmt::{Debug, Formatter};
use std::{future::Future, sync::Arc};
use tokio_util::sync::CancellationToken;

#[cfg(all(feature = "tasks", not(feature = "legacy-spec")))]
#[cfg(all(feature = "tasks", feature = "legacy-spec"))]
use crate::types::{
    GetTaskPayloadRequestParams, ListTasksRequestParams, ListTasksResult, Task, TaskPayload,
};

pub mod api;
pub mod batch;
mod calls;
mod capabilities;
mod handler;
mod listen;
#[cfg(not(feature = "legacy-spec"))]
mod mrtr;
mod notification_handler;
pub mod options;
mod setup;
pub mod subscribe;
#[cfg(not(feature = "legacy-spec"))]
pub mod subscription;
#[cfg(feature = "tasks")]
pub mod task;

pub use batch::BatchBuilder;
#[cfg(not(feature = "legacy-spec"))]
pub use subscription::{Subscription, SubscriptionEnd};
#[cfg(feature = "tasks")]
pub use task::TaskBuilder;

/// Represents an MCP client app
pub struct Client {
    /// MCP client options.
    options: McpOptions,

    /// Capabilities supported by the connected server.
    server_capabilities: Option<ServerCapabilities>,

    /// Implementation information of the connected server.
    server_info: std::sync::OnceLock<Implementation>,

    /// A [`CancellationToken`] that cancels transport background processes.
    cancellation_token: Option<CancellationToken>,

    /// Request handler
    handler: Option<RequestHandler>,
}

impl Debug for Client {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("options", &self.options)
            .field("server_capabilities", &self.server_capabilities)
            .field("server_info", &self.server_info.get())
            .finish()
    }
}

impl Default for Client {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    /// Returns whether the server supports task-augmented tools
    #[inline]
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    fn is_server_support_call_tool_with_tasks(&self) -> bool {
        self.server_tasks_capability()
            .and_then(|c| c.requests)
            .and_then(|r| r.tools)
            .is_some_and(|t| t.call.is_some())
    }

    /// Sends a request to the MCP server
    #[inline]
    pub(super) async fn send_request(&self, req: Request) -> Result<Response, Error> {
        // Checked at the send seam rather than in `call_tool`, so every way of
        // reaching a tool -- the plain call, the task builder -- goes past it.
        #[cfg(all(feature = "http-client", not(feature = "legacy-spec")))]
        if let Some(err) = self.blocked_tool_error(&req) {
            return Err(err);
        }

        #[cfg(not(feature = "legacy-spec"))]
        {
            // A legacy peer (dual-mode fallback) never speaks MRTR -- its
            // requests take the plain path, elicitation rides the legacy
            // server-push channel instead.
            if self.is_legacy_peer() {
                return self.plain_send_request(req).await;
            }
            self.run_with_mrtr(req).await
        }
        #[cfg(feature = "legacy-spec")]
        {
            self.plain_send_request(req).await
        }
    }

    /// Sends a request without the MRTR loop.
    #[inline]
    pub(super) async fn plain_send_request(&self, req: Request) -> Result<Response, Error> {
        let resp = self
            .handler
            .as_ref()
            .ok_or_else(|| Error::new(ErrorCode::InternalError, "Connection closed"))?
            .send_request(req)
            .await?;
        #[cfg(not(feature = "legacy-spec"))]
        self.record_server_info(&resp);
        Ok(resp)
    }

    /// Creates a [`BatchBuilder`] for sending multiple requests in a single batch.
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///     client.connect().await?;
    ///
    ///     let responses = client
    ///         .batch()
    ///         .list_tools()
    ///         .list_prompts()
    ///         .send()
    ///         .await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub fn batch(&self) -> BatchBuilder<'_> {
        BatchBuilder {
            client: self,
            items: Vec::new(),
        }
    }

    /// Returns a [`TaskBuilder`] for constructing a task-augmented request.
    #[cfg(feature = "tasks")]
    #[deprecated(since = "0.7.0", note = "use `client.tools().as_task()`")]
    #[inline]
    pub fn task(&self) -> TaskBuilder<'_> {
        self.tools().as_task()
    }

    /// Sends a batch of messages to the MCP server and awaits all responses.
    ///
    /// Items that are [`MessageEnvelope::Request`] each get a response slot in
    /// the returned `Vec`, in the same order they appear in `items`.
    /// [`MessageEnvelope::Notification`] items are sent fire-and-forget and
    /// produce no slot.
    ///
    /// All in-flight requests are awaited concurrently; a failure in one
    /// does not cancel the others.
    ///
    /// The client numbers every request it sends, and these too: on the wire
    /// each request carries an id this client generated, and each response
    /// comes back carrying the id the caller gave its request. An id chosen by
    /// the caller could repeat one still owed an answer -- a request given up
    /// on, another batch in flight -- and the answer would then reach the wrong
    /// waiter.
    ///
    /// # Errors
    /// Returns [`Error`] if the client is not connected, the batch is empty,
    /// or any response channel is closed or times out.
    pub async fn call_batch(&self, items: Vec<MessageEnvelope>) -> Result<Vec<Response>, Error> {
        let mut caller_ids = Vec::new();
        let items = items
            .into_iter()
            .map(|envelope| match envelope {
                MessageEnvelope::Request(mut req) => {
                    caller_ids.push(std::mem::replace(&mut req.id, self.generate_id()?));
                    Ok(MessageEnvelope::Request(req))
                }
                other => Ok(other),
            })
            .collect::<Result<Vec<_>, Error>>()?;

        // One response per request, in order: the caller's ids go back on them.
        let responses = self.send_numbered_batch(items).await?;
        Ok(responses
            .into_iter()
            .zip(caller_ids)
            .map(|(resp, id)| resp.set_id(id))
            .collect())
    }

    /// [`Self::call_batch`] for requests this client already numbered, such as
    /// [`BatchBuilder`]'s, whose ids also seed their progress tokens.
    pub(super) async fn send_numbered_batch(
        &self,
        items: Vec<MessageEnvelope>,
    ) -> Result<Vec<Response>, Error> {
        // One blocked tool fails the whole batch, the same as a duplicate id
        // does: the batch is one write, and there is no way to drop a single
        // entry from it without silently changing what the caller asked for.
        #[cfg(all(feature = "http-client", not(feature = "legacy-spec")))]
        if let Some(err) = items.iter().find_map(|env| match env {
            MessageEnvelope::Request(req) => self.blocked_tool_error(req),
            _ => None,
        }) {
            return Err(err);
        }

        // Under MCP 2026-07-28 a batched request may elicit just like a single send, so
        // the batch is driven through the same MRTR retry loop (see
        // `run_batch_with_mrtr`) rather than returning the protocol-intermediate
        // `input_required` result as final. A legacy peer (dual-mode
        // fallback) never speaks MRTR and takes the plain path.
        #[cfg(not(feature = "legacy-spec"))]
        {
            if !self.is_legacy_peer() {
                return self.run_batch_with_mrtr(items).await;
            }
        }
        let handler = self
            .handler
            .as_ref()
            .ok_or_else(|| Error::new(ErrorCode::InternalError, "Connection closed"))?;

        let slots = handler.send_batch(items).await?;

        collect_batch_responses(slots, handler)
            .await
            .into_iter()
            .collect()
    }

    /// Sends a response to the MCP server
    ///
    /// Only the legacy profile has server->client requests to answer.
    #[inline]
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    async fn send_response(&self, req: Response) -> Result<(), Error> {
        self.handler
            .as_ref()
            .ok_or_else(|| Error::new(ErrorCode::InternalError, "Connection closed"))?
            .send_response(req)
            .await;
        Ok(())
    }

    /// Sends a notification to the MCP server
    #[inline]
    async fn send_notification(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(), Error> {
        let notification = Notification::new(method, params);
        self.handler
            .as_ref()
            .ok_or_else(|| Error::new(ErrorCode::InternalError, "Connection closed"))?
            .send_notification(notification)
            .await
    }

    #[cfg(feature = "tracing")]
    fn register_tracing_notification_handlers(&mut self) {
        use crate::types::notification::commands::*;

        self.subscribe(MESSAGE, Self::default_notification_handler);
        self.subscribe(STDERR, Self::default_notification_handler);
        self.subscribe(PROGRESS, Self::default_notification_handler);
    }

    #[cfg(feature = "tracing")]
    async fn default_notification_handler(notification: Notification) {
        notification.write();
    }

    /// Generates a new [`RequestId`]
    #[inline]
    fn generate_id(&self) -> Result<RequestId, Error> {
        self.handler
            .as_ref()
            .ok_or_else(|| Error::new(ErrorCode::InternalError, "Connection closed"))
            .map(|h| h.next_id())
    }

    /// Cancels the transport and clears connection state without sending a
    /// notification. Used when initialization fails after the transport has
    /// already been started (e.g. protocol version mismatch in `init()`).
    #[inline]
    pub(super) fn cancel_transport(&mut self) {
        if let Some(token) = self.cancellation_token.take() {
            token.cancel();
        }
        self.handler = None;
    }

    #[inline]
    fn wait_for_shutdown_signal(&mut self) {
        if let Some(token) = self.cancellation_token.clone() {
            shared::wait_for_shutdown_signal(token);
        };
    }

    #[cfg(feature = "tasks")]
    pub(crate) fn ensure_tasks_supported(&self) {
        assert!(
            self.is_client_supports_tasks(),
            "Client does not support task-augmented requests. You may configure it with `Client::with_options(|opt| opt.with_tasks(...))` method."
        );

        assert!(
            self.is_server_supports_tasks(),
            "Server does not support task-augmented requests."
        );
    }
}

/// Awaits a batch's per-request receivers concurrently, returning one result
/// per receiver in input order, with the same per-request timeout and slot
/// release as a single [`RequestHandler::send_request`].
///
/// Each request's slot guard moves into the future awaiting its reply, so the
/// slot goes back when that wait ends -- answered, timed out, or abandoned
/// with the whole batch by a caller that dropped it. Uses `join_all` (not
/// `try_join_all`) so one failed request does not cut the others' waits short.
async fn collect_batch_responses(
    slots: Vec<(
        tokio::sync::oneshot::Receiver<shared::PendingResponse>,
        shared::QueuedRequestGuard<'_>,
    )>,
    handler: &RequestHandler,
) -> Vec<Result<Response, Error>> {
    use futures_util::future::join_all;

    let request_timeout = handler.timeout();
    let token = handler.cancellation();
    let sender = handler.sender();

    let futures = slots.into_iter().map(|(rx, slot)| {
        let token = token.clone();
        // The batch is out, so each of its requests can be cancelled: one the
        // caller stops waiting for tells the server, as a single request does.
        let mut abandon = handler::Abandon::new(Some((slot.id().clone(), sender.clone())));
        async move {
            let _slot = slot;
            tokio::select! {
                biased;
                // The transport died (or a shutdown signal cancelled it)
                // -- no response is coming for any receiver.
                _ = token.cancelled() => {
                    abandon.disarm();
                    Err(Error::new(ErrorCode::InternalError, "Connection closed"))
                }
                result = tokio::time::timeout(request_timeout, rx) => match result {
                    Ok(Ok(shared::PendingResponse::Response(resp))) => {
                        abandon.disarm();
                        Ok(resp)
                    }
                    Ok(Ok(shared::PendingResponse::Timeout)) | Err(_) => {
                        abandon.timed_out();
                        Err(Error::new(ErrorCode::Timeout, "Batch request timed out"))
                    }
                    Ok(Err(_)) => {
                        abandon.disarm();
                        Err(Error::new(
                            ErrorCode::InternalError,
                            "Response channel closed",
                        ))
                    }
                }
            }
        }
    });

    join_all(futures).await
}

/// Describes a handler a client answers a server-initiated request with --
/// [`Client::map_sampling`] and [`Client::map_elicitation`].
///
/// Implemented for every function taking the request's `P` and returning
/// something that converts into the result `O`, in both shapes a handler can
/// take: an **asynchronous** one returning a future of such a value
/// ([`marker::Async`], the default) and a **synchronous** one returning the
/// value itself ([`marker::Immediate`]), plus the same handler moved onto the
/// blocking pool by [`blocking`](crate::blocking).
///
/// `M` records which shape a given function is and is inferred at the
/// registration site. See [`crate::marker`] for which shape to write.
///
/// # Examples
/// ```no_run
/// use neva::{Client, types::elicitation::{ElicitRequestParams, ElicitResult}};
///
/// # #[tokio::main]
/// # async fn main() {
/// let mut client = Client::new();
///
/// // asynchronous
/// client.map_elicitation(|_params: ElicitRequestParams| async { ElicitResult::accept() });
///
/// // synchronous
/// client.map_elicitation(|_params: ElicitRequestParams| ElicitResult::accept());
/// # }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid handler for `{P}`",
    label = "not a client handler",
    note = "a handler takes `{P}` and returns either a value that converts into `{O}` or a \
            future of one"
)]
pub trait ClientHandler<P, O, M = marker::Async>: Clone + Send + Sync + 'static {
    /// Calls the handler, boxing whatever it returns into the shape the client
    /// stores.
    fn call(&self, params: P) -> BoxFuture<'static, O>;
}

impl<F, Fut, P, O> ClientHandler<P, O, marker::Async> for F
where
    F: Fn(P) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future + Send + 'static,
    Fut::Output: Into<O>,
    P: Send + 'static,
{
    #[inline]
    fn call(&self, params: P) -> BoxFuture<'static, O> {
        let handler = self.clone();
        Box::pin(async move { handler(params).await.into() })
    }
}

// The synchronous shape. `R: Into<O>` is what keeps this impl and the
// asynchronous one apart during selection -- a future does not convert into a
// sampling or elicitation result, and neither result is a future.
impl<F, R, P, O> ClientHandler<P, O, marker::Immediate> for F
where
    F: Fn(P) -> R + Clone + Send + Sync + 'static,
    R: Into<O> + Send + 'static,
    O: Send + 'static,
{
    #[inline]
    fn call(&self, params: P) -> BoxFuture<'static, O> {
        Box::pin(std::future::ready((self)(params).into()))
    }
}

// The same handler moved onto Tokio's blocking pool by `neva::blocking`.
impl<F, R, P, O> ClientHandler<P, O, marker::Immediate> for BlockingFn<F>
where
    F: Fn(P) -> R + Clone + Send + Sync + 'static,
    R: Into<O> + Send + 'static,
    P: Send + 'static,
    O: Send + 'static,
{
    #[inline]
    fn call(&self, params: P) -> BoxFuture<'static, O> {
        let handler = self.0.clone();
        Box::pin(async move {
            BlockingCall {
                handle: tokio::task::spawn_blocking(move || handler(params)),
            }
            .await
            .into()
        })
    }
}

#[inline]
fn make_handler<F, P, O, M>(handler: F) -> Handler<P, O>
where
    F: ClientHandler<P, O, M>,
    P: Send + 'static,
    O: Send + 'static,
{
    Arc::new(move |params: P| handler.call(params))
}

type Handler<P, O> =
    Arc<dyn Fn(P) -> std::pin::Pin<Box<dyn Future<Output = O> + Send>> + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;

    /// Requests take `&self`, so one client can serve many tasks at once --
    /// which only holds while the client is `Sync` and each request's future
    /// is `Send`. Nothing else would notice either one being lost.
    #[test]
    fn a_client_can_be_shared_across_tasks() {
        fn shared<T: Send + Sync>() {}
        fn sendable<F: Future + Send>(_: F) {}

        shared::<Client>();

        let client = Client::new();
        sendable(client.tools().list(None));
        sendable(client.tools().call("add", [("a", 1), ("b", 2)]));
        sendable(client.resources().list(None));
        sendable(client.resources().read("file:///readme.md"));
        sendable(client.prompts().list(None));
        sendable(client.prompts().get("summarise", ()));
        #[cfg(feature = "legacy-spec")]
        sendable(client.ping());
        sendable(client.batch().list_tools().send());
    }

    #[tokio::test]
    async fn call_batch_requires_connected_client() {
        let client = Client::new();
        let result = client.call_batch(vec![]).await;
        assert!(
            result.is_err(),
            "disconnected client should return an error"
        );
    }

    #[tokio::test]
    async fn map_elicitation_accepts_every_handler_shape() {
        let mut client = Client::new();

        // Asynchronous, synchronous and offloaded register through the same
        // method; the marker is inferred from each signature.
        client.map_elicitation(|_params: ElicitRequestParams| async { ElicitResult::accept() });
        client.map_elicitation(|_params: ElicitRequestParams| ElicitResult::accept());
        client.map_elicitation(crate::blocking(|_params: ElicitRequestParams| {
            ElicitResult::accept()
        }));

        // The last one registered is the one that answers, and it answers
        // through the blocking pool.
        let handler = client
            .options
            .elicitation_handler
            .clone()
            .expect("a registered handler");
        let result = handler(ElicitRequestParams::form("Proceed?").into()).await;

        assert!(result.is_accepted());
    }

    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn batch_injects_rc_client_meta_per_request() {
        use serde_json::json;

        let mut client = Client::new();
        // Registering an elicitation handler makes the client declare
        // `clientCapabilities.elicitation = true`.
        client.map_elicitation(|_params: ElicitRequestParams| async { ElicitResult::accept() });

        let req = Request::new(
            Some(RequestId::Number(1)),
            "tools/call",
            Some(json!({ "name": "greet", "arguments": {} })),
        );
        let mut items = vec![
            MessageEnvelope::Request(req),
            // Notifications must be left untouched.
            MessageEnvelope::Notification(Notification::new("notifications/progress", None)),
        ];

        client.apply_client_meta_to_batch(&mut items);

        let MessageEnvelope::Request(req) = &items[0] else {
            panic!("first item must be a request");
        };
        let meta = &req.params.as_ref().expect("params present")["_meta"];
        // Without this injection a batched eliciting tools/call is rejected as
        // if the client did not support elicitation. The spec spells a declared
        // capability as an object, not a boolean.
        assert_eq!(
            meta["io.modelcontextprotocol/clientCapabilities"]["elicitation"],
            json!({})
        );
        assert!(meta["io.modelcontextprotocol/clientInfo"].is_object());
        assert_eq!(
            meta["io.modelcontextprotocol/protocolVersion"],
            json!("2026-07-28")
        );

        // The notification carries no params/_meta.
        let MessageEnvelope::Notification(notif) = &items[1] else {
            panic!("second item must be a notification");
        };
        assert!(notif.params.is_none());
    }

    /// With no handshake under 2026-07-28, a request's `_meta` is the only
    /// place a server can see an extension declared -- so the map `initialize`
    /// carries rides every request too, beside the MRTR flags.
    #[cfg(all(feature = "apps", not(feature = "legacy-spec")))]
    #[test]
    fn declared_extensions_ride_every_request() {
        use crate::types::{APP_MIME_TYPE, APPS_EXTENSION_ID};
        use serde_json::json;

        const CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";

        let mut client = Client::new().with_options(|opt| opt.with_apps());
        client.map_elicitation(|_params: ElicitRequestParams| ElicitResult::accept());

        let mut req = Request::new(Some(RequestId::Number(1)), "tools/call", None::<()>);
        client.apply_client_meta(&mut req, None, None);

        let caps = &req.params.as_ref().expect("params present")["_meta"][CAPABILITIES];
        assert_eq!(
            caps["extensions"][APPS_EXTENSION_ID],
            json!({ "mimeTypes": [APP_MIME_TYPE] })
        );
        assert_eq!(caps["elicitation"], json!({}), "the MRTR flags stay flat");

        // And a client that declared none writes no map at all.
        let mut req = Request::new(Some(RequestId::Number(2)), "tools/call", None::<()>);
        Client::new().apply_client_meta(&mut req, None, None);

        let caps = &req.params.as_ref().expect("params present")["_meta"][CAPABILITIES];
        assert!(
            caps.is_object(),
            "the capabilities object is still required"
        );
        assert!(caps.get("extensions").is_none(), "got: {caps}");
    }

    /// An MRTR retry states its answers where the spec puts them: on the
    /// params, beside `name` and `arguments`. They used to go into `_meta`,
    /// where no other implementation looks for them.
    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn a_retry_states_its_answers_in_the_params() {
        use serde_json::json;

        let client = Client::new();
        let mut req = Request::new(
            Some(RequestId::Number(2)),
            "tools/call",
            Some(json!({ "name": "greet", "arguments": {} })),
        );

        let answers: crate::types::mrtr::InputResponses =
            [("who".to_string(), json!({ "action": "accept" }))]
                .into_iter()
                .collect();
        client.apply_client_meta(&mut req, Some(answers), Some("v1.0.sealed".into()));

        let params = req.params.as_ref().expect("params present");
        assert_eq!(params["requestState"], json!("v1.0.sealed"));
        assert_eq!(params["inputResponses"]["who"]["action"], json!("accept"));
        // The params it is a retry *of* are untouched...
        assert_eq!(params["name"], json!("greet"));
        // ...and `_meta` keeps the envelope without duplicating the answers.
        assert!(params["_meta"]["io.modelcontextprotocol/clientInfo"].is_object());
        assert!(params["_meta"].get("inputResponses").is_none());
        assert!(params["_meta"].get("requestState").is_none());
    }

    /// A configured trace-context provider is invoked during 2026-07-28 metadata
    /// assembly, so `_meta.traceparent`/`tracestate` reach the wire alongside
    /// `clientInfo` -- for both single sends and (via the same path) batches.
    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn apply_client_meta_injects_trace_context() {
        use crate::client::options::TraceContext;
        use serde_json::json;

        let client = Client::new().with_options(|o| {
            o.with_trace_context_provider(|| {
                Some(TraceContext {
                    traceparent: "tp".into(),
                    tracestate: Some("ts".into()),
                    baggage: Some("bg".into()),
                })
            })
        });

        let mut req = Request::new(
            Some(RequestId::Number(1)),
            "tools/call",
            Some(json!({ "name": "greet", "arguments": {} })),
        );
        client.apply_client_meta(&mut req, None, None);

        let meta = &req.params.as_ref().expect("params present")["_meta"];
        assert_eq!(meta["traceparent"], json!("tp"));
        assert_eq!(meta["tracestate"], json!("ts"));
        // Trace context is assembled alongside the rest of the 2026-07-28 metadata.
        assert!(meta["io.modelcontextprotocol/clientInfo"].is_object());
    }

    /// With no provider installed, no trace fields are emitted.
    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn apply_client_meta_omits_trace_context_without_provider() {
        use serde_json::json;

        let client = Client::new();
        let mut req = Request::new(
            Some(RequestId::Number(1)),
            "tools/call",
            Some(json!({ "name": "greet", "arguments": {} })),
        );
        client.apply_client_meta(&mut req, None, None);

        let meta = &req.params.as_ref().expect("params present")["_meta"];
        assert!(meta.get("traceparent").is_none());
        assert!(meta.get("tracestate").is_none());
    }
}

/// A request a caller gives up on, against a real server that never answers it.
#[cfg(all(test, feature = "http-server-volga", feature = "http-client"))]
mod abandoned_request_tests {
    use super::*;
    use crate::App;
    use std::time::Duration;

    fn pick_free_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("local addr").port();
        drop(listener);
        port
    }

    /// A caller dropping the future -- an outer `timeout`, a lost `select!`
    /// branch -- runs none of the request's own error paths. The slot still
    /// comes back at once, and the answer that arrives later finds no waiter:
    /// the id is never sent again, so it cannot reach another request.
    #[tokio::test(flavor = "multi_thread")]
    async fn dropping_a_call_releases_its_slot() {
        let addr = format!("127.0.0.1:{}", pick_free_port());

        let release = Arc::new(tokio::sync::Notify::new());
        let gate = release.clone();
        let mut app = App::new()
            .without_greeting()
            .with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")));
        app.map_tool("hold", move || {
            let gate = gate.clone();
            async move {
                gate.notified().await;
                "held".to_string()
            }
        });
        app.map_tool("quick", || async { "quick".to_string() });
        let server = tokio::spawn(async move { app.run().await });
        tokio::time::sleep(Duration::from_millis(300)).await;

        let mut client = Client::new().with_options(|o| {
            o.with_http(|h| h.bind(&addr).with_endpoint("/mcp"))
                // Far longer than this test waits: the slot must be released
                // by the dropped future, not by the request's own timeout.
                .with_timeout(Duration::from_secs(30))
        });
        client.connect().await.expect("connect");

        let queued = |client: &Client| client.handler.as_ref().expect("connected").pending().len();
        let idle = queued(&client);

        assert!(
            tokio::time::timeout(Duration::from_millis(300), client.tools().call("hold", ()))
                .await
                .is_err(),
            "the tool holds its answer, so the outer timeout must fire"
        );
        assert_eq!(queued(&client), idle, "a dropped call releases its slot");

        // The late answer is dropped, and the next call gets its own.
        release.notify_one();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let next = client.tools().call("quick", ()).await.expect("call");
        assert!(
            serde_json::to_string(&next)
                .expect("json")
                .contains("quick")
        );
        assert_eq!(queued(&client), idle);

        server.abort();
    }

    /// Deadlines are otherwise checked only when another request goes out or
    /// an answer comes in. A client that goes quiet after a burst would keep
    /// every slot its caller gave up on, and every deadline scheduled, until
    /// its next request -- so a connected client sweeps on its own.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_idle_client_sweeps_expired_slots() {
        let addr = format!("127.0.0.1:{}", pick_free_port());

        let mut app = App::new()
            .without_greeting()
            .with_options(|o| o.with_http(|h| h.bind(&addr).with_endpoint("/mcp")));
        app.map_tool("stall", || async { std::future::pending::<String>().await });
        app.map_tool("quick", || async { "quick".to_string() });
        let server = tokio::spawn(async move { app.run().await });
        tokio::time::sleep(Duration::from_millis(300)).await;

        let mut client = Client::new().with_options(|o| {
            o.with_http(|h| h.bind(&addr).with_endpoint("/mcp"))
                .with_timeout(Duration::from_millis(200))
        });
        client.connect().await.expect("connect");
        let queued = |client: &Client| client.handler.as_ref().expect("connected").pending().len();
        let scheduled = |client: &Client| {
            client
                .handler
                .as_ref()
                .expect("connected")
                .pending()
                .scheduled()
        };
        let idle = queued(&client);

        client.tools().call("quick", ()).await.expect("answered");
        let _ =
            tokio::time::timeout(Duration::from_millis(50), client.tools().call("stall", ())).await;
        assert!(client.tools().call("stall", ()).await.is_err(), "timed out");

        // Quiet from here on: nothing goes out, nothing comes in.
        tokio::time::sleep(Duration::from_millis(900)).await;

        assert_eq!(queued(&client), idle, "expired slots are swept");
        assert_eq!(scheduled(&client), 0, "and so are their deadlines");

        server.abort();
    }
}

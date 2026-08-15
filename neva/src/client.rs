//! Utilities for the MCP client

use crate::error::{Error, ErrorCode};
use crate::shared;
use crate::transport::Transport;
use crate::types::Root;
use crate::types::sampling::{CreateMessageRequestParams, CreateMessageResult, SamplingHandler};
use crate::types::{
    CallToolRequestParams, CallToolResponse, GetPromptRequestParams, GetPromptResult,
    Implementation, ListPromptsRequestParams, ListPromptsResult,
    ListResourceTemplatesRequestParams, ListResourceTemplatesResult, ListResourcesRequestParams,
    ListResourcesResult, ListToolsRequestParams, ListToolsResult, MessageEnvelope,
    ReadResourceRequestParams, ReadResourceResult, Request, RequestId, RequestParamsMeta, Response,
    ServerCapabilities, Uri,
    cursor::Cursor,
    elicitation::{ElicitRequestParams, ElicitResult, ElicitationHandler},
    notification::Notification,
    resource::{SubscribeRequestParams, UnsubscribeRequestParams},
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

#[cfg(feature = "tasks")]
use crate::types::TaskMetadata;
#[cfg(all(feature = "tasks", not(feature = "legacy-spec")))]
#[cfg(all(feature = "tasks", feature = "legacy-spec"))]
use crate::types::{
    GetTaskPayloadRequestParams, ListTasksRequestParams, ListTasksResult, Task, TaskPayload,
};

/// How many `tools/list` pages the `HeaderMismatch` recovery will walk looking
/// for the tool it was sent back for.
///
/// The traversal ends on its own at a page without a `nextCursor`; this is the
/// bound for a server that never stops handing them out, which would otherwise
/// keep a single failed call walking forever with nothing above it able to see.
#[cfg(all(feature = "http-client", not(feature = "legacy-spec")))]
const MAX_REFRESH_PAGES: usize = 64;

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
    server_info: Option<Implementation>,

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
            .field("server_info", &self.server_info)
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
    pub(super) async fn send_request(&mut self, req: Request) -> Result<Response, Error> {
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
    pub(super) async fn plain_send_request(&mut self, req: Request) -> Result<Response, Error> {
        let resp = self
            .handler
            .as_mut()
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
    pub fn batch(&mut self) -> BatchBuilder<'_> {
        BatchBuilder {
            client: self,
            items: Vec::new(),
        }
    }

    /// Returns a [`TaskBuilder`] for constructing a task-augmented request.
    ///
    /// Chain setters such as [`TaskBuilder::with_ttl`] to configure the task,
    /// then call [`TaskBuilder::call_tool`] to execute.
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
    ///     let result = client
    ///         .task()
    ///         .with_ttl(5000)
    ///         .call_tool("echo", [("message", "Hello MCP!")])
    ///         .await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    #[cfg(feature = "tasks")]
    pub fn task(&mut self) -> TaskBuilder<'_> {
        TaskBuilder {
            client: self,
            metadata: TaskMetadata::default(),
        }
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
    /// # Errors
    /// Returns [`Error`] if the client is not connected, the batch is empty,
    /// or any response channel is closed or times out.
    pub async fn call_batch(
        &mut self,
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
            .as_mut()
            .ok_or_else(|| Error::new(ErrorCode::InternalError, "Connection closed"))?;

        let request_timeout = handler.timeout();
        let pending = handler.pending().clone();
        let token = handler.cancellation();
        let receivers = handler.send_batch(items).await?;

        collect_batch_responses(receivers, &pending, request_timeout, token)
            .await
            .into_iter()
            .collect()
    }

    /// Sends a response to the MCP server
    ///
    /// Only the legacy profile has server->client requests to answer.
    #[inline]
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    async fn send_response(&mut self, req: Response) -> Result<(), Error> {
        self.handler
            .as_mut()
            .ok_or_else(|| Error::new(ErrorCode::InternalError, "Connection closed"))?
            .send_response(req)
            .await;
        Ok(())
    }

    /// Sends a notification to the MCP server
    #[inline]
    async fn send_notification(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(), Error> {
        let notification = Notification::new(method, params);
        self.handler
            .as_mut()
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
/// per receiver in input order, with the same per-request timeout and pending
/// cleanup as a single [`RequestHandler::send_request`].
///
/// Uses `join_all` (not `try_join_all`) so every future runs to completion: the
/// timeout-cleanup branch (`pending.pop`) executes for each timed-out request
/// even when another request in the same batch has already failed.
async fn collect_batch_responses(
    receivers: Vec<(
        RequestId,
        tokio::sync::oneshot::Receiver<crate::shared::PendingResponse>,
    )>,
    pending: &crate::shared::RequestQueue,
    request_timeout: std::time::Duration,
    token: tokio_util::sync::CancellationToken,
) -> Vec<Result<Response, Error>> {
    use futures_util::future::join_all;

    let futures = receivers.into_iter().map(|(id, rx)| {
        let pending = pending.clone();
        let token = token.clone();
        async move {
            tokio::select! {
                biased;
                // The transport died (or a shutdown signal cancelled it)
                // -- no response is coming for any receiver.
                _ = token.cancelled() => {
                    let _ = pending.pop(&id);
                    Err(Error::new(ErrorCode::InternalError, "Connection closed"))
                }
                result = tokio::time::timeout(request_timeout, rx) => match result {
                    Ok(Ok(crate::shared::PendingResponse::Response(resp))) => Ok(resp),
                    Ok(Ok(crate::shared::PendingResponse::Timeout)) => {
                        Err(Error::new(ErrorCode::Timeout, "Batch request timed out"))
                    }
                    Ok(Err(_)) => Err(Error::new(
                        ErrorCode::InternalError,
                        "Response channel closed",
                    )),
                    Err(_) => {
                        let _ = pending.pop(&id);
                        Err(Error::new(ErrorCode::Timeout, "Batch request timed out"))
                    }
                }
            }
        }
    });

    join_all(futures).await
}

#[inline]
fn make_handler<F, R, P, O>(handler: F) -> Handler<P, O>
where
    F: Fn(P) -> R + Clone + Send + Sync + 'static,
    R: Future + Send,
    R::Output: Into<O>,
    P: Send + 'static,
    O: Send + 'static,
{
    Arc::new(move |params: P| {
        let handler = handler.clone();
        Box::pin(async move { handler(params).await.into() })
    })
}

type Handler<P, O> =
    Arc<dyn Fn(P) -> std::pin::Pin<Box<dyn Future<Output = O> + Send>> + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn call_batch_requires_connected_client() {
        let mut client = Client::new();
        let result = client.call_batch(vec![]).await;
        assert!(
            result.is_err(),
            "disconnected client should return an error"
        );
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

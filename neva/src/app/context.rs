//! Server runtime context utilities

use super::{
    handler::RequestHandler,
    options::{McpOptions, RuntimeMcpOptions},
};
use crate::error::{Error, ErrorCode};
use crate::transport::Sender;
use crate::types::notification::Notification;
#[cfg(feature = "legacy-spec")]
use crate::types::root::{ListRootsRequestParams, ListRootsResult};
#[cfg(feature = "legacy-spec")]
use crate::types::sampling::{CreateMessageRequestParams, CreateMessageResult};
#[cfg(not(feature = "legacy-spec"))]
use crate::types::{
    RequestId, SubscriptionFilter, SubscriptionsAcknowledgedNotificationParams,
    SubscriptionsListenResult,
};
use crate::{
    middleware::{MwContext, Next},
    shared::{IntoArgs, RequestQueue},
    transport::TransportProtoSender,
    types::{
        CallToolRequestParams, CallToolResponse, GetPromptRequestParams, GetPromptResult, Message,
        Prompt, ReadResourceRequestParams, ReadResourceResult, Request, Resource, Response, Tool,
        ToolResult, ToolUse, Uri,
        elicitation::{ElicitRequestParams, ElicitResult},
        resource::SubscribeRequestParams,
    },
};

// `RequestId` is only referenced by the server->client request paths (legacy
// elicitation/sampling) and the task API; under the stateless 2026-07-28 build without
// tasks it is unused -- including in tests, whose `RequestId` uses live in
// modules carrying those very same gates.
#[cfg(any(feature = "legacy-spec", feature = "tasks"))]
#[cfg(feature = "legacy-spec")]
use crate::types::RequestId;
use std::{
    collections::HashMap,
    fmt::{Debug, Formatter},
    sync::Arc,
    time::Duration,
};
use tokio::time::timeout;

#[cfg(feature = "http-server")]
use crate::transport::http::core::auth::{validate_permissions, validate_roles};
#[cfg(all(feature = "tasks", feature = "legacy-spec"))]
use crate::types::{
    CancelTaskRequestParams, Cursor, GetTaskPayloadRequestParams, GetTaskRequestParams,
    ListTasksRequestParams, ListTasksResult, TaskPayload,
};
#[cfg(feature = "tasks")]
use crate::{
    shared::Either,
    types::{CreateTaskResult, Task, tool::TaskSupport},
};
#[cfg(feature = "tasks")]
#[cfg(feature = "legacy-spec")]
use serde::de::DeserializeOwned;
#[cfg(feature = "di")]
use volga_di::Container;
#[cfg(feature = "http-server")]
use {crate::auth::Claims, http::HeaderMap};

#[cfg(feature = "tasks")]
pub(crate) type ToolOrTaskResponse = Either<CreateTaskResult, CallToolResponse>;

type RequestHandlers = HashMap<String, RequestHandler<Response>>;

mod effects;
mod interaction;
mod listen;
mod primitives;
mod tasks;

#[cfg(not(feature = "legacy-spec"))]
pub(crate) use effects::MrtrCtx;
#[cfg(all(not(feature = "legacy-spec"), feature = "tasks"))]
pub use tasks::TaskContext;
#[cfg(all(not(feature = "legacy-spec"), feature = "tasks"))]
pub(crate) use tasks::TaskExec;

/// The execution substrate the current 2026-07-28 dispatch is running on. Elicitation
/// and the `once`/`memo`/`on_commit` helpers dispatch on this so the stateless
/// MRTR machinery and the stateful task machinery never mix: a bare call uses
/// `requestState` re-run, a task-augmented call suspends a live background
/// future. The two are different substrates, not one with a flag.
#[cfg(not(feature = "legacy-spec"))]
#[derive(Clone, Default)]
pub(crate) enum ExecMode {
    /// Not an elicitable dispatch (or no special execution context).
    #[default]
    None,

    /// Stateless MRTR call: progress lives in the encrypted `requestState` and
    /// the handler re-runs each round.
    Mrtr(Arc<MrtrCtx>),

    /// Stateful task-augmented call: the tool runs in a background future that
    /// genuinely suspends on the task tracker. Carries no MRTR key /
    /// `requestState` / replay log -- the task tracker is the held state.
    #[cfg(feature = "tasks")]
    Task(Arc<TaskExec>),
}

/// Represents a Server runtime
#[derive(Clone)]
pub(crate) struct ServerRuntime {
    /// Represents MCP server options
    options: RuntimeMcpOptions,

    /// Represents registered request handlers
    handlers: Arc<RequestHandlers>,

    /// Represents a queue of pending requests
    pending: RequestQueue,

    /// Represents a sender that depends on selected transport protocol
    sender: TransportProtoSender,

    /// Global middlewares entrypoint
    mw_start: Option<Next>,

    /// Represents a DI container
    #[cfg(feature = "di")]
    pub(crate) container: Container,
}

/// Represents MCP Request Context
#[derive(Clone)]
pub struct Context {
    /// Represents current session id
    pub session_id: Option<uuid::Uuid>,

    /// Represents HTTP headers of the current request
    #[cfg(feature = "http-server")]
    pub headers: HeaderMap,

    /// Type-erased JWT/auth claims of the current request.
    ///
    /// Inserted by the HTTP engine. Any type implementing [`Claims`]
    /// works -- neva's `DefaultClaims`, or a custom claims struct from a
    /// custom engine adapter.
    #[cfg(feature = "http-server")]
    pub(crate) claims: Option<Arc<dyn Claims>>,

    /// Represents MCP server options
    pub(crate) options: RuntimeMcpOptions,

    /// Represents a queue of pending requests
    ///
    /// Only read by [`Context::send_request`] (server->client requests), which
    /// the stateless 2026-07-28 build does not use.
    #[cfg_attr(not(feature = "legacy-spec"), allow(dead_code))]
    pending: RequestQueue,

    /// Represents a sender that depends on selected transport protocol
    ///
    /// See [`Self::pending`] for why this is dead under MCP 2026-07-28.
    #[cfg_attr(not(feature = "legacy-spec"), allow(dead_code))]
    sender: TransportProtoSender,

    /// Represents a timeout for the current request
    timeout: Duration,

    /// Execution substrate for this dispatch (set by the server dispatch layer:
    /// `Mrtr` for a stateless elicitable call, `Task` for a background
    /// task-augmented call, `None` otherwise).
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) exec: ExecMode,

    /// What the caller declared it can answer, read off this request's `_meta`.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) client_capabilities: crate::types::mrtr::ClientMrtrCapabilities,

    /// Represents a DI scope
    #[cfg(feature = "di")]
    pub(crate) scope: Option<Container>,
}

impl Debug for Context {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context")
            .field("session_id", &self.session_id)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl ServerRuntime {
    /// Creates a new server runtime
    pub(crate) fn new(
        sender: TransportProtoSender,
        mut options: McpOptions,
        handlers: RequestHandlers,
        #[cfg(feature = "di")] container: Container,
    ) -> Self {
        let middlewares = options.middlewares.take();
        let request_timeout = options.request_timeout;
        Self {
            pending: RequestQueue::new(request_timeout),
            handlers: Arc::new(handlers),
            options: options.into_runtime(),
            mw_start: middlewares.and_then(|mw| mw.compose()),
            sender,
            #[cfg(feature = "di")]
            container,
        }
    }

    /// Provides a [`RuntimeMcpOptions`]
    pub(crate) fn options(&self) -> RuntimeMcpOptions {
        self.options.clone()
    }

    /// Provides the current connections sender
    pub(crate) fn sender(&self) -> TransportProtoSender {
        self.sender.clone()
    }

    /// Provides the counter of messages currently inside the middleware
    /// pipeline, which the shutdown drain waits on.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) fn in_flight(&self) -> Arc<std::sync::atomic::AtomicUsize> {
        self.options.in_flight()
    }

    /// Returns a clone of this runtime with its sender replaced by `sender`.
    ///
    /// Used by `execute_batch` to give each batch request an intercepted sender
    /// so responses are captured into a channel instead of sent to the transport.
    pub(crate) fn with_sender(mut self, sender: TransportProtoSender) -> Self {
        self.sender = sender;
        self
    }

    /// Provides a hash map of registered request handlers
    pub(crate) fn request_handlers(&self) -> Arc<RequestHandlers> {
        self.handlers.clone()
    }

    /// Creates a new MCP request [`Context`]
    #[cfg(not(feature = "http-server"))]
    pub(crate) fn context(&self, session_id: Option<uuid::Uuid>) -> Context {
        Context {
            session_id,
            pending: self.pending.clone(),
            sender: self.sender.clone(),
            options: self.options.clone(),
            timeout: self.options.request_timeout,
            #[cfg(not(feature = "legacy-spec"))]
            exec: ExecMode::None,
            #[cfg(not(feature = "legacy-spec"))]
            client_capabilities: Default::default(),
            #[cfg(feature = "di")]
            scope: None,
        }
    }

    /// Creates a new MCP request [`Context`]
    #[cfg(feature = "http-server")]
    pub(crate) fn context(
        &self,
        session_id: Option<uuid::Uuid>,
        headers: HeaderMap,
        claims: Option<Arc<dyn Claims>>,
    ) -> Context {
        Context {
            session_id,
            headers,
            claims,
            pending: self.pending.clone(),
            sender: self.sender.clone(),
            options: self.options.clone(),
            timeout: self.options.request_timeout,
            #[cfg(not(feature = "legacy-spec"))]
            exec: ExecMode::None,
            #[cfg(not(feature = "legacy-spec"))]
            client_capabilities: Default::default(),
            #[cfg(feature = "di")]
            scope: None,
        }
    }

    /// Provides a "queue" of pending requests
    pub(crate) fn pending_requests(&self) -> &RequestQueue {
        &self.pending
    }

    /// Starts the middleware pipeline
    #[inline]
    pub(crate) async fn execute(self, msg: Message) {
        if let Some(mw_start) = self.mw_start.clone() {
            mw_start(MwContext::msg(msg, self)).await;
        }
    }
}

impl Context {
    /// Sends the elicitation request to the client
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(feature = "serve-macros")] {
    /// use neva::{
    ///     Context,
    ///     error::Error,
    ///     types::elicitation::ElicitRequestParams,
    ///     tool
    /// };
    ///
    /// #[tool]
    /// async fn generate_poem(mut ctx: Context, _topic: String) -> Result<String, Error> {
    ///     let params = ElicitRequestParams::new("What is the poem mood you'd like?")
    ///         .with_required("mood", "string");
    ///     let result = ctx.elicit(params).await?;
    ///     Ok(format!("{:?}", result.content))
    /// }
    /// # }
    /// ```
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    pub async fn elicit(&mut self, params: ElicitRequestParams) -> Result<ElicitResult, Error> {
        let related_task = params.related_task();

        if let Some(related_task) = related_task {
            let task_id = related_task.id;
            let mut id = task_id
                .as_str()
                .parse::<RequestId>()
                .expect("Invalid task id");

            if let Some(session_id) = self.session_id {
                id = id.concat(session_id.into());
            }

            let receiver = self.pending.push(&id);

            self.options.tasks.set_result(&task_id, params);
            self.options.tasks.require_input(&task_id);

            let resp = match timeout(self.timeout, receiver).await {
                Ok(Ok(crate::shared::PendingResponse::Response(resp))) => resp,
                Ok(Ok(crate::shared::PendingResponse::Timeout)) => {
                    self.options.tasks.fail(&task_id);
                    return Err(Error::new(ErrorCode::Timeout, "Request timed out"));
                }
                Ok(Err(_)) => {
                    self.options.tasks.fail(&task_id);
                    return Err(Error::new(
                        ErrorCode::InternalError,
                        "Response channel closed",
                    ));
                }
                Err(_) => {
                    _ = self.pending.pop(&id);
                    self.options.tasks.fail(&task_id);
                    return Err(Error::new(ErrorCode::Timeout, "Request timed out"));
                }
            };

            self.options.tasks.reset(&task_id);

            return resp.into_result();
        }

        let method = crate::types::elicitation::commands::CREATE;
        let is_task_aug = params.is_task_augmented();
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(params),
        );

        self.send_maybe_task_augmented_request(req, is_task_aug)
            .await
    }

    /// Notifies the client that the elicitation with the `id` has been completed
    ///
    /// Removed in MCP 2026-07-28; available only under `legacy-spec`.
    #[cfg(feature = "legacy-spec")]
    pub async fn complete_elicitation(&mut self, id: impl Into<String>) -> Result<(), Error> {
        let params = serde_json::to_value(
            crate::types::elicitation::ElicitationCompleteParams::new(id),
        )
        .ok();
        self.send_notification(crate::types::elicitation::commands::COMPLETE, params)
            .await
    }

    /// Sends notification that a task with `id` was changed.
    ///
    /// The payload is the same `DetailedTask` `tasks/get` answers with, since
    /// `notifications/tasks` is what saves a subscribed client from polling:
    /// a bare status would send it back to `tasks/get` for the
    /// `inputRequests` / `result` / `error` the notification is meant to carry.
    #[cfg(all(feature = "tasks", not(feature = "legacy-spec")))]
    pub async fn task_changed(&mut self, id: &str) -> Result<(), Error> {
        let task = self.options.tasks.get_state(id)?;
        let params = serde_json::to_value(task).ok();
        self.send_notification(crate::types::task::commands::STATUS, params)
            .await
    }

    /// Sends notification that a task with `id` was changed.
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    pub async fn task_changed(&mut self, id: &str) -> Result<(), Error> {
        let task = self.options.tasks.get_status(id)?;
        let params = serde_json::to_value(task).ok();
        self.send_notification(crate::types::task::commands::STATUS, params)
            .await
    }

    /// Applies earlier defined scopes to the current context.
    #[inline]
    #[cfg(feature = "di")]
    pub fn with_scope(mut self, scope: Container) -> Self {
        self.scope = Some(scope);
        self
    }

    /// Resolves a service and returns a cloned instance.
    /// `T` must implement `Clone` otherwise
    /// use resolve_shared method that returns a shared pointer.
    #[inline]
    #[cfg(feature = "di")]
    pub fn resolve<T: Send + Sync + Clone + 'static>(&self) -> Result<T, Error> {
        self.scope
            .as_ref()
            .ok_or_else(|| Error::new(ErrorCode::InternalError, "DI scope is not set"))?
            .resolve::<T>()
            .map_err(Into::into)
    }

    /// Resolves a service and returns a shared pointer
    #[inline]
    #[cfg(feature = "di")]
    pub fn resolve_shared<T: Send + Sync + 'static>(&self) -> Result<Arc<T>, Error> {
        self.scope
            .as_ref()
            .ok_or_else(|| Error::new(ErrorCode::InternalError, "DI scope is not set"))?
            .resolve_shared::<T>()
            .map_err(Into::into)
    }

    #[inline]
    #[cfg(feature = "http-server")]
    fn validate_claims(
        &self,
        roles: Option<&[String]>,
        permissions: Option<&[String]>,
    ) -> Result<(), Error> {
        let claims = self.claims.as_deref();
        validate_roles(claims, roles)?;
        validate_permissions(claims, permissions)?;
        Ok(())
    }

    #[inline]
    #[cfg(feature = "tasks")]
    fn ensure_tool_augmentation_support(
        &self,
        task_support: Option<TaskSupport>,
    ) -> Result<(), Error> {
        if !self.options.is_task_augmented_tool_call_supported() {
            return Err(Error::new(
                ErrorCode::MethodNotFound,
                "Server does not support task augmented tool calls",
            ));
        }
        let Some(task_support) = task_support else {
            return Err(Error::new(
                ErrorCode::MethodNotFound,
                "Tool does not support task augmented calls",
            ));
        };
        if task_support == TaskSupport::Forbidden {
            return Err(Error::new(
                ErrorCode::MethodNotFound,
                "Tool forbid task augmented calls",
            ));
        }
        Ok(())
    }

    #[inline]
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    async fn send_maybe_task_augmented_request<T: DeserializeOwned>(
        &mut self,
        req: Request,
        is_task_aug: bool,
    ) -> Result<T, Error> {
        if is_task_aug {
            let result = self.send_request(req).await?.into_result()?;

            crate::shared::wait_to_completion(self, result).await
        } else {
            self.send_request(req).await?.into_result()
        }
    }

    /// Sends a [`Request`] to a client
    ///
    /// Server->client requests (legacy elicitation/sampling/roots and the task
    /// API). The stateless 2026-07-28 transport has no out-of-band server->client
    /// channel, so this is unused there.
    #[inline]
    #[cfg_attr(not(feature = "legacy-spec"), allow(dead_code))]
    async fn send_request(&mut self, mut req: Request) -> Result<Response, Error> {
        if let Some(session_id) = self.session_id {
            req.session_id = Some(session_id);
        }

        let id = req.full_id();
        let receiver = self.pending.push(&id);
        if let Err(err) = self.sender.send(req.into()).await {
            let _ = self.pending.pop(&id);
            return Err(err);
        }
        self.pending.activate(&id);

        match timeout(self.timeout, receiver).await {
            Ok(Ok(crate::shared::PendingResponse::Response(resp))) => Ok(resp),
            Ok(Ok(crate::shared::PendingResponse::Timeout)) => {
                Err(Error::new(ErrorCode::Timeout, "Request timed out"))
            }
            Ok(Err(_)) => Err(Error::new(
                ErrorCode::InternalError,
                "Response channel closed",
            )),
            Err(_) => {
                _ = self.pending.pop(&id);
                Err(Error::new(ErrorCode::Timeout, "Request timed out"))
            }
        }
    }

    /// Sends a notification to a client.
    ///
    /// Under MCP 2026-07-28 the only server->client channel is a
    /// `subscriptions/listen` stream, so a subscribable notification
    /// (`tools`/`prompts`/`resources` list-changed and `resources/updated`) is
    /// fanned out to every stream whose filter admits it. The rest -- progress,
    /// task status, elicitation -- are request-scoped and have no subscription
    /// to travel on.
    ///
    /// With a
    /// [`NotificationBus`](crate::app::notification_bus::NotificationBus)
    /// installed the fan-out goes through the bus instead, and comes back to
    /// every instance -- this one included -- through the drain task
    /// `App::run` spawns. That is one code path rather than two, at the cost of
    /// a round trip the local case does not otherwise pay; the default is no
    /// bus, which delivers straight to the local registry.
    #[inline]
    async fn send_notification(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(), Error> {
        #[cfg(not(feature = "legacy-spec"))]
        {
            if !crate::types::subscription::is_subscribable(method) {
                // Not a type a client can subscribe to. Surface it once at debug so
                // a server author who expects a push isn't silently misled.
                #[cfg(feature = "tracing")]
                tracing::debug!(
                    method,
                    "notification is not deliverable under MCP 2026-07-28: no subscription carries this type"
                );
                return Ok(());
            }

            match self.options.notification_bus() {
                // `params` is moved rather than cloned here, and `method` is a
                // `&'static str` constant at every call site, so a bus costs
                // one small allocation on top of its own round trip.
                Some(bus) => {
                    bus.publish(crate::app::notification_bus::BusNotification::new(
                        method, params,
                    ))
                    .await
                }
                None => {
                    self.options
                        .subscriptions()
                        .broadcast(method, params.as_ref());
                }
            }
            Ok(())
        }
        #[cfg(feature = "legacy-spec")]
        {
            let mut notification = Notification::new(method, params);
            if let Some(session_id) = self.session_id {
                notification.session_id = Some(session_id);
            }
            self.sender.send(notification.into()).await
        }
    }
}

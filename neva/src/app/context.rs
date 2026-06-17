//! Server runtime context utilities

use super::{
    handler::RequestHandler,
    options::{McpOptions, RuntimeMcpOptions},
};
use crate::error::{Error, ErrorCode};
use crate::transport::Sender;
#[cfg(not(feature = "proto-2026-07-28-rc"))]
use crate::types::notification::Notification;
#[cfg(not(feature = "proto-2026-07-28-rc"))]
use crate::types::root::{ListRootsRequestParams, ListRootsResult};
#[cfg(not(feature = "proto-2026-07-28-rc"))]
use crate::types::sampling::{CreateMessageRequestParams, CreateMessageResult};
use crate::{
    middleware::{MwContext, Next},
    shared::{IntoArgs, RequestQueue},
    transport::TransportProtoSender,
    types::{
        CallToolRequestParams, CallToolResponse, GetPromptRequestParams, GetPromptResult, Message,
        Prompt, ReadResourceRequestParams, ReadResourceResult, Request, Resource,
        Response, Tool, ToolResult, ToolUse, Uri,
        elicitation::{ElicitRequestParams, ElicitResult, ElicitationCompleteParams},
        resource::SubscribeRequestParams,
    },
};
// `RequestId` is only referenced by the server→client request paths (non-RC
// elicitation/sampling) and the task API; under the stateless RC build without
// tasks it is unused.
#[cfg(any(not(feature = "proto-2026-07-28-rc"), feature = "tasks", test))]
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
#[cfg(feature = "tasks")]
use crate::{
    shared::Either,
    types::{
        CancelTaskRequestParams, CreateTaskResult, Cursor, GetTaskPayloadRequestParams,
        GetTaskRequestParams, ListTasksRequestParams, ListTasksResult, Task, TaskPayload,
        tool::TaskSupport,
    },
};
#[cfg(feature = "tasks")]
use serde::de::DeserializeOwned;
#[cfg(feature = "di")]
use volga_di::Container;
#[cfg(feature = "http-server")]
use {crate::auth::Claims, http::HeaderMap};

#[cfg(feature = "tasks")]
pub(crate) type ToolOrTaskResponse = Either<CreateTaskResult, CallToolResponse>;

type RequestHandlers = HashMap<String, RequestHandler<Response>>;

/// Boxed deferred-commit future (see [`Context::on_commit`]).
#[cfg(feature = "proto-2026-07-28-rc")]
pub(crate) type CommitFut =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + Send>>;

/// Per-dispatch MRTR state: the replay log of answers available to the
/// handler this round, the single input it newly requested, plus the
/// `once`/`memo`/`on_commit` bookkeeping.
#[cfg(feature = "proto-2026-07-28-rc")]
#[derive(Default)]
pub(crate) struct MrtrCtx {
    /// Answers available this round (prior answers decoded from
    /// `requestState`, merged with this round's `inputResponses`).
    pub(crate) answers: std::collections::HashMap<String, crate::types::elicitation::ElicitResult>,
    /// The newly-requested input (v1: at most one), recorded on a cache miss.
    pub(crate) pending: std::sync::Mutex<Option<(String, ElicitRequestParams)>>,
    /// Whether the client declared elicitation support this round.
    pub(crate) elicitation_allowed: bool,
    /// Cached `ctx.memo` values (seeded from `requestState`, grown on miss).
    pub(crate) memos: std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>,
    /// Executed `ctx.once` keys (seeded from `requestState`, grown on run).
    pub(crate) effects: std::sync::Mutex<std::collections::HashSet<String>>,
    /// Deferred `ctx.on_commit` futures, rebuilt each round, drained on the
    /// final (non-`input_required`) round. Never serialized.
    pub(crate) commits: std::sync::Mutex<Vec<CommitFut>>,
}

#[cfg(feature = "proto-2026-07-28-rc")]
impl std::fmt::Debug for MrtrCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MrtrCtx")
            .field("answers", &self.answers)
            .field("pending", &self.pending)
            .field("elicitation_allowed", &self.elicitation_allowed)
            .field("memos", &self.memos)
            .field("effects", &self.effects)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "proto-2026-07-28-rc")]
impl MrtrCtx {
    /// Returns the cached answer for `key`, or records the request and returns
    /// the MRTR "input required" sentinel error to unwind the handler.
    pub(crate) fn resolve(
        &self,
        key: String,
        params: ElicitRequestParams,
    ) -> Result<ElicitResult, Error> {
        if let Some(answer) = self.answers.get(&key) {
            return Ok(answer.clone());
        }
        if let Ok(mut pending) = self.pending.lock() {
            *pending = Some((key, params));
        }
        Err(Error::input_required())
    }

    /// Returns whether a `once` effect key has already run this chain.
    pub(crate) fn effect_seen(&self, key: &str) -> bool {
        self.effects
            .lock()
            .map(|e| e.contains(key))
            .unwrap_or(false)
    }

    /// Records a `once` effect key as run.
    pub(crate) fn record_effect(&self, key: String) {
        if let Ok(mut e) = self.effects.lock() {
            e.insert(key);
        }
    }

    /// Returns the cached `memo` value for `key`, if present.
    pub(crate) fn cached_memo(&self, key: &str) -> Option<serde_json::Value> {
        self.memos.lock().ok().and_then(|m| m.get(key).cloned())
    }

    /// Stores a `memo` value.
    pub(crate) fn store_memo(&self, key: String, value: serde_json::Value) {
        if let Ok(mut m) = self.memos.lock() {
            m.insert(key, value);
        }
    }

    /// Registers a deferred commit future.
    pub(crate) fn push_commit(&self, fut: CommitFut) {
        if let Ok(mut c) = self.commits.lock() {
            c.push(fut);
        }
    }
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
    /// works — neva's `DefaultClaims`, or a custom claims struct from a
    /// custom engine adapter.
    #[cfg(feature = "http-server")]
    pub(crate) claims: Option<Arc<dyn Claims>>,

    /// Represents MCP server options
    pub(crate) options: RuntimeMcpOptions,

    /// Represents a queue of pending requests
    ///
    /// Only read by [`Context::send_request`] (server→client requests), which
    /// the stateless RC build does not use.
    #[cfg_attr(feature = "proto-2026-07-28-rc", allow(dead_code))]
    pending: RequestQueue,

    /// Represents a sender that depends on selected transport protocol
    ///
    /// See [`Self::pending`] for why this is dead under the RC.
    #[cfg_attr(feature = "proto-2026-07-28-rc", allow(dead_code))]
    sender: TransportProtoSender,

    /// Represents a timeout for the current request
    timeout: Duration,

    /// MRTR per-dispatch state (set by the server dispatch layer for the
    /// supported methods; `None` otherwise).
    #[cfg(feature = "proto-2026-07-28-rc")]
    pub(crate) mrtr: Option<Arc<MrtrCtx>>,

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
            #[cfg(feature = "proto-2026-07-28-rc")]
            mrtr: None,
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
            #[cfg(feature = "proto-2026-07-28-rc")]
            mrtr: None,
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
    /// Returns a list of all available tools
    pub async fn tools(&self) -> Vec<Tool> {
        self.options.tools.values().await
    }

    /// Finds a tool by `name`
    pub async fn find_tool(&self, name: &str) -> Option<Tool> {
        self.options.tools.get(name).await
    }

    /// Returns a list of tools by name.
    /// If some tools requested in `names` are missing, they won't be in the result list.
    pub async fn find_tools(&self, names: impl IntoIterator<Item = &str>) -> Vec<Tool> {
        futures_util::future::join_all(names.into_iter().map(|name| self.options.tools.get(name)))
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    /// Initiates a tool call once a [`ToolUse`] request received from assistant
    /// withing a sampling window.
    ///
    /// For multiple [`ToolUse`] requests, use the [`Context::use_tools`] method.
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", not(feature = "proto-2026-07-28-rc")))] {
    /// use neva::prelude::*;
    ///
    /// #[tool]
    /// async fn analyze_weather(ctx: Context, city: String) -> Result<(), Error> {
    ///     let args = ("city", city);
    ///     let weather = ctx.use_tool(ToolUse::new("get_weather", args)).await;
    ///
    ///     // do something with the weather result
    ///
    /// # Ok(())
    /// }
    ///
    /// #[tool]
    /// async fn get_weather(city: String) -> String {
    ///     // ...
    ///
    ///     format!("Sunny in {city}")
    /// }
    /// # }
    /// ```
    pub async fn use_tool(&self, tool: ToolUse) -> ToolResult {
        let id = tool.id.clone();
        let res = self.clone().call_tool(tool.into()).await;
        match res {
            Ok(res) => ToolResult::new(id, res),
            Err(err) => ToolResult::error(id, err),
        }
    }

    /// Initiates a parallel tool calls for multiple [`ToolUse`] requests.
    ///
    /// For a single [`ToolUse`] use the [`Context::use_tool`] method.
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(feature = "server-macros")] {
    /// use neva::prelude::*;
    ///
    /// #[tool]
    /// async fn analyze_weather(ctx: Context) -> Result<(), Error> {
    ///     let weather = ctx.use_tools([
    ///         ToolUse::new("get_weather", ("city", "London")),
    ///         ToolUse::new("get_weather", ("city", "Paris"))
    ///     ]).await;
    ///     
    ///     // do something with the weather result
    ///
    /// # Ok(())
    /// }
    /// # }
    /// ```
    pub async fn use_tools<I>(&self, tools: I) -> Vec<ToolResult>
    where
        I: IntoIterator<Item = ToolUse>,
    {
        futures_util::future::join_all(tools.into_iter().map(|t| self.use_tool(t))).await
    }

    /// Gets the prompt by name
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", not(feature = "proto-2026-07-28-rc")))] {
    /// use neva::prelude::*;
    ///
    /// #[tool]
    /// async fn analyze_weather(ctx: Context, city: String) -> Result<(), Error> {
    ///     let prompt = ctx.prompt("get_weather", ("city", city)).await?;
    ///
    ///     // do something with the prompt
    ///
    /// # Ok(())
    /// }
    ///
    /// #[prompt]
    /// async fn get_weather(city: String) -> PromptMessage {
    ///     PromptMessage::user()
    ///         .with(format!("What's the weather in {city}"))
    /// }
    /// # }
    /// ```
    pub async fn prompt<N, Args>(&self, name: N, args: Args) -> Result<GetPromptResult, Error>
    where
        N: Into<String>,
        Args: IntoArgs,
    {
        let params = GetPromptRequestParams {
            name: name.into(),
            args: args.into_args(),
            meta: None,
        };
        self.clone().get_prompt(params).await
    }

    /// Reads a resource content
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", not(feature = "proto-2026-07-28-rc")))] {
    /// use neva::prelude::*;
    ///
    /// #[tool]
    /// async fn summarize_document(ctx: Context, doc_uri: Uri) -> Result<(), Error> {
    ///     let doc = ctx.resource(doc_uri).await?;
    ///
    ///     // do something with the doc
    ///
    /// # Ok(())
    /// }
    ///
    /// #[resource(uri = "file://{name}")]
    /// async fn get_doc(name: String) -> TextResourceContents {
    ///     // read the doc
    ///
    /// # TextResourceContents::new("", "")
    /// }
    /// # }
    /// ```
    pub async fn resource(&self, uri: impl Into<Uri>) -> Result<ReadResourceResult, Error> {
        let uri = uri.into();
        let params = ReadResourceRequestParams::from(uri);
        self.clone().read_resource(params).await
    }

    /// Adds a new resource and notifies clients
    pub async fn add_resource(&mut self, res: impl Into<Resource>) -> Result<(), Error> {
        let res: Resource = res.into();
        self.options.resources.insert(res.name.clone(), res).await?;

        if self.options.is_resource_list_changed_supported() {
            self.send_notification(crate::types::resource::commands::LIST_CHANGED, None)
                .await
        } else {
            Ok(())
        }
    }

    /// Removes a resource and notifies clients
    pub async fn remove_resource(
        &mut self,
        uri: impl Into<Uri>,
    ) -> Result<Option<Resource>, Error> {
        let removed = self.options.resources.remove(&uri.into()).await?;

        if removed.is_some() && self.options.is_resource_list_changed_supported() {
            self.send_notification(crate::types::resource::commands::LIST_CHANGED, None)
                .await?;
        }

        Ok(removed)
    }

    /// Sends a notification that the resource with the `uri` has been updated
    pub async fn resource_updated(&mut self, uri: impl Into<Uri>) -> Result<(), Error> {
        if !self.options.is_resource_subscription_supported() {
            return Err(Error::new(
                ErrorCode::MethodNotFound,
                "Server does not support sending resource/updated notifications",
            ));
        }

        let uri = uri.into();
        if self.is_subscribed(&uri) {
            let params = serde_json::to_value(SubscribeRequestParams::from(uri)).ok();
            self.send_notification(crate::types::resource::commands::UPDATED, params)
                .await
        } else {
            Ok(())
        }
    }

    /// Adds a subscription to the resource with the [`Uri`]
    pub fn subscribe_to_resource(&mut self, uri: impl Into<Uri>) {
        self.options.resource_subscriptions.insert(uri.into());
    }

    /// Removes a subscription to the resource with the [`Uri`]
    pub fn unsubscribe_from_resource(&mut self, uri: &Uri) {
        self.options.resource_subscriptions.remove(uri);
    }

    /// Returns `true` if there is a subscription to changes of the resource with the [`Uri`]
    pub fn is_subscribed(&self, uri: &Uri) -> bool {
        self.options.resource_subscriptions.contains(uri)
    }

    /// Adds a new prompt and notifies clients
    pub async fn add_prompt(&mut self, prompt: Prompt) -> Result<(), Error> {
        self.options
            .prompts
            .insert(prompt.name.clone(), prompt)
            .await?;

        if self.options.is_prompts_list_changed_supported() {
            self.send_notification(crate::types::prompt::commands::LIST_CHANGED, None)
                .await
        } else {
            Ok(())
        }
    }

    /// Removes a prompt and notifies clients
    pub async fn remove_prompt(
        &mut self,
        name: impl Into<String>,
    ) -> Result<Option<Prompt>, Error> {
        let removed = self.options.prompts.remove(&name.into()).await?;

        if removed.is_some() && self.options.is_prompts_list_changed_supported() {
            self.send_notification(crate::types::prompt::commands::LIST_CHANGED, None)
                .await?;
        }

        Ok(removed)
    }

    /// Adds a new prompt and notifies clients
    pub async fn add_tool(&mut self, tool: Tool) -> Result<(), Error> {
        self.options.tools.insert(tool.name.clone(), tool).await?;

        if self.options.is_tools_list_changed_supported() {
            self.send_notification(crate::types::tool::commands::LIST_CHANGED, None)
                .await
        } else {
            Ok(())
        }
    }

    /// Removes a tool and notifies clients
    pub async fn remove_tool(&mut self, name: impl Into<String>) -> Result<Option<Tool>, Error> {
        let removed = self.options.tools.remove(&name.into()).await?;

        if removed.is_some() && self.options.is_tools_list_changed_supported() {
            self.send_notification(crate::types::tool::commands::LIST_CHANGED, None)
                .await?;
        }

        Ok(removed)
    }

    #[inline]
    pub(crate) async fn read_resource(
        self,
        params: ReadResourceRequestParams,
    ) -> Result<ReadResourceResult, Error> {
        let opt = self.options.clone();
        match opt.read_resource(&params.uri) {
            Some((handler, args)) => {
                #[cfg(feature = "http-server")]
                {
                    let template = opt.resources_templates.get(&handler.template).await;
                    self.validate_claims(
                        template.as_ref().and_then(|t| t.roles.as_deref()),
                        template.as_ref().and_then(|t| t.permissions.as_deref()),
                    )
                }?;
                handler
                    .call(params.with_args(args).with_context(self).into())
                    .await
            }
            _ => Err(Error::from(ErrorCode::RESOURCE_NOT_FOUND)),
        }
    }

    #[inline]
    pub(crate) async fn get_prompt(
        self,
        params: GetPromptRequestParams,
    ) -> Result<GetPromptResult, Error> {
        match self.options.get_prompt(&params.name).await {
            None => Err(Error::new(ErrorCode::InvalidParams, "Prompt not found")),
            Some(prompt) => {
                #[cfg(feature = "http-server")]
                self.validate_claims(prompt.roles.as_deref(), prompt.permissions.as_deref())?;
                prompt.call(params.with_context(self).into()).await
            }
        }
    }

    #[inline]
    pub(crate) async fn call_tool(
        self,
        params: CallToolRequestParams,
    ) -> Result<CallToolResponse, Error> {
        match self.options.get_tool(&params.name).await {
            None => Err(Error::new(ErrorCode::InvalidParams, "Tool not found")),
            Some(tool) => {
                #[cfg(feature = "http-server")]
                self.validate_claims(tool.roles.as_deref(), tool.permissions.as_deref())?;
                tool.call(params.with_context(self).into()).await
            }
        }
    }

    #[inline]
    #[cfg(feature = "tasks")]
    pub(crate) async fn call_tool_with_task(
        self,
        params: CallToolRequestParams,
    ) -> Result<ToolOrTaskResponse, Error> {
        match self.options.get_tool(&params.name).await {
            None => Err(Error::new(ErrorCode::InvalidParams, "Tool not found")),
            Some(tool) => {
                #[cfg(feature = "http-server")]
                self.validate_claims(tool.roles.as_deref(), tool.permissions.as_deref())?;

                let task_support = tool.task_support();
                if let Some(task_meta) = params.task {
                    self.ensure_tool_augmentation_support(task_support)?;

                    let task = Task::from(task_meta);
                    let handle = self.options.track_task(task.clone());

                    let opt = self.options.clone();
                    let task_id = task.id.clone();
                    tokio::spawn(async move {
                        tokio::select! {
                            result = tool.call(params
                                .with_task(&task_id)
                                .with_context(self).into()) => {
                                let resp = match result {
                                    Ok(result) => {
                                        opt.tasks.complete(&task_id);
                                        result
                                    },
                                    Err(err) => {
                                        opt.tasks.fail(&task_id);
                                        CallToolResponse::error(err)
                                    }
                                };
                                handle.set_result(resp);
                            },
                            _ = handle.cancelled() => {}
                        }
                    });

                    Ok(Either::Left(CreateTaskResult::new(task)))
                } else if task_support.is_some_and(|ts| ts == TaskSupport::Required) {
                    Err(Error::new(
                        ErrorCode::MethodNotFound,
                        "Tool required task augmented call",
                    ))
                } else {
                    tool.call(params.with_context(self).into())
                        .await
                        .map(Either::Right)
                }
            }
        }
    }

    /// Requests a list of available roots from a client
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", not(feature = "proto-2026-07-28-rc")))] {
    /// use neva::{Context, error::Error, tool};
    ///
    /// #[tool]
    /// async fn handle_roots(mut ctx: Context) -> Result<(), Error> {
    ///     let roots = ctx.list_roots().await?;
    ///
    ///     // do something with roots
    ///
    /// # Ok(())
    /// }
    /// # }
    /// ```
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
    pub async fn list_roots(&mut self) -> Result<ListRootsResult, Error> {
        let method = crate::types::root::commands::LIST;
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(ListRootsRequestParams::default()),
        );

        self.send_request(req).await?.into_result()
    }

    /// Sends the sampling request to the client
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(feature = "server-macros")] {
    /// use neva::{
    ///     Context,
    ///     error::Error,
    ///     types::sampling::CreateMessageRequestParams,
    ///     tool
    /// };
    ///
    /// #[tool]
    /// async fn generate_poem(mut ctx: Context, topic: String) -> Result<String, Error> {
    ///     let params = CreateMessageRequestParams::new()
    ///         .with_message(format!("Write a short poem about {topic}"))
    ///         .with_sys_prompt("You are a talented poet who writes concise, evocative verses.");
    ///
    ///     let result = ctx.sample(params).await?;
    ///     Ok(format!("{:?}", result.content))
    /// }
    /// # }
    /// ```
    #[cfg(all(not(feature = "tasks"), not(feature = "proto-2026-07-28-rc")))]
    pub async fn sample(
        &mut self,
        params: CreateMessageRequestParams,
    ) -> Result<CreateMessageResult, Error> {
        let method = crate::types::sampling::commands::CREATE;
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(params),
        );

        self.send_request(req).await?.into_result()
    }

    /// Sends the sampling request to the client
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(feature = "server-macros")] {
    /// use neva::{
    ///     Context,
    ///     error::Error,
    ///     types::sampling::CreateMessageRequestParams,
    ///     tool
    /// };
    ///
    /// #[tool]
    /// async fn generate_poem(mut ctx: Context, topic: String) -> Result<String, Error> {
    ///     let params = CreateMessageRequestParams::new()
    ///         .with_message(format!("Write a short poem about {topic}"))
    ///         .with_sys_prompt("You are a talented poet who writes concise, evocative verses.");
    ///
    ///     let result = ctx.sample(params).await?;
    ///     Ok(format!("{:?}", result.content))
    /// }
    /// # }
    /// ```
    #[cfg(all(feature = "tasks", not(feature = "proto-2026-07-28-rc")))]
    pub async fn sample(
        &mut self,
        params: CreateMessageRequestParams,
    ) -> Result<CreateMessageResult, Error> {
        let method = crate::types::sampling::commands::CREATE;
        let is_task_aug = params.task.is_some();
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(params),
        );

        self.send_maybe_task_augmented_request(req, is_task_aug)
            .await
    }

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
    #[cfg(all(not(feature = "tasks"), not(feature = "proto-2026-07-28-rc")))]
    pub async fn elicit(&mut self, params: ElicitRequestParams) -> Result<ElicitResult, Error> {
        let method = crate::types::elicitation::commands::CREATE;
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(params),
        );

        self.send_request(req).await?.into_result()
    }

    /// Requests elicitation input from the client (MRTR, `proto-2026-07-28-rc`).
    ///
    /// On the first dispatch the answer for `key` is absent: the request is
    /// recorded and an internal sentinel error is returned, which the server
    /// converts into an `InputRequiredResult`. When the client retries with
    /// the answer, this handler re-runs and the call returns the cached
    /// [`ElicitResult`].
    ///
    /// **Important:** code before an `elicit` point re-executes on every
    /// round-trip — keep it side-effect-free.
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", feature = "proto-2026-07-28-rc"))] {
    /// use neva::{Context, error::Error, types::elicitation::ElicitRequestParams, tool};
    /// #[tool]
    /// async fn greet(mut ctx: Context) -> Result<String, Error> {
    ///     let params = ElicitRequestParams::form("Your name?")
    ///         .with_required("name", "string")
    ///         .into();
    ///     let res = ctx.elicit("name", params).await?;
    ///     Ok(format!("{:?}", res.content))
    /// }
    /// # }
    /// ```
    #[cfg(feature = "proto-2026-07-28-rc")]
    pub async fn elicit(
        &mut self,
        key: impl Into<String>,
        params: ElicitRequestParams,
    ) -> Result<ElicitResult, Error> {
        match self.mrtr.as_ref() {
            Some(mrtr) => mrtr.resolve(key.into(), params),
            None => Err(Error::new(
                ErrorCode::InvalidRequest,
                "elicitation is not available for this request",
            )),
        }
    }

    /// Runs `effect` at most once across MRTR rounds (`proto-2026-07-28-rc`).
    ///
    /// On a replay (the key was recorded in a prior round) the future is
    /// dropped unpolled and `Ok(false)` is returned. On a miss the future is
    /// awaited; on success the key is recorded and `Ok(true)` is returned, on
    /// failure the error propagates and the key is **not** recorded (so the
    /// next round retries).
    ///
    /// Sync work lives inside a non-awaiting `async {}` block.
    ///
    /// # Durability
    /// The effect runs *before* the `requestState` recording it is durably
    /// acknowledged by the client — it is at-most-once within a single
    /// `requestState` chain, **not** globally exactly-once. For non-idempotent
    /// side effects, pass a stable idempotency key to the downstream system.
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", feature = "proto-2026-07-28-rc"))] {
    /// # use neva::{Context, error::Error};
    /// # async fn f(ctx: Context) -> Result<(), Error> {
    /// ctx.once("emit_metric", async { Ok(()) }).await?;
    /// # Ok(()) }
    /// # }
    /// ```
    #[cfg(feature = "proto-2026-07-28-rc")]
    pub async fn once<F>(&self, key: impl Into<String>, effect: F) -> Result<bool, Error>
    where
        F: std::future::Future<Output = Result<(), Error>>,
    {
        let key = key.into();
        match self.mrtr.as_ref() {
            None => {
                effect.await?;
                Ok(true)
            }
            Some(mrtr) => {
                if mrtr.effect_seen(&key) {
                    return Ok(false);
                }
                effect.await?;
                mrtr.record_effect(key);
                Ok(true)
            }
        }
    }

    /// Computes `compute` at most once across MRTR rounds and caches the
    /// serialized value in `requestState` (`proto-2026-07-28-rc`).
    ///
    /// On a replay the cached value is deserialized and returned (the future is
    /// dropped unpolled). On a miss the future is awaited, the value serialized
    /// and stored, and returned. A failed compute is not cached.
    ///
    /// Caching a value grows `requestState`; prefer [`Context::once`] when the
    /// result isn't needed later. See [`crate::App::with_max_state_bytes`].
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", feature = "proto-2026-07-28-rc"))] {
    /// # use neva::{Context, error::Error};
    /// # async fn f(ctx: Context) -> Result<(), Error> {
    /// let n: i32 = ctx.memo("answer", async { Ok(42) }).await?;
    /// # let _ = n; Ok(()) }
    /// # }
    /// ```
    #[cfg(feature = "proto-2026-07-28-rc")]
    pub async fn memo<T, F>(&self, key: impl Into<String>, compute: F) -> Result<T, Error>
    where
        T: serde::Serialize + serde::de::DeserializeOwned,
        F: std::future::Future<Output = Result<T, Error>>,
    {
        let key = key.into();
        match self.mrtr.as_ref() {
            None => compute.await,
            Some(mrtr) => {
                if let Some(value) = mrtr.cached_memo(&key) {
                    return serde_json::from_value(value).map_err(Error::from);
                }
                let value = compute.await?;
                mrtr.store_memo(key, serde_json::to_value(&value).map_err(Error::from)?);
                Ok(value)
            }
        }
    }

    /// Registers `effect` to run **exactly once**, when the handler reaches its
    /// final (non-`input_required`) result (`proto-2026-07-28-rc`).
    ///
    /// Commits are awaited in registration order before the final response is
    /// sent; the first `Err` becomes the response error. They do **not** run on
    /// intermediate `input_required` rounds, nor when the handler errors.
    ///
    /// The future is stored in the shared dispatch state, so it must be
    /// `Send + 'static` — capture by `move`. Called outside an MRTR dispatch,
    /// it is a no-op.
    ///
    /// # Durability
    /// "Exactly once" means once per successfully-completed flow, not globally
    /// idempotent — a client that abandons and restarts the flow runs it again.
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(feature = "server-macros", feature = "proto-2026-07-28-rc"))] {
    /// # use neva::{Context, error::Error};
    /// # async fn f(ctx: Context) {
    /// ctx.on_commit(async move { Ok(()) });
    /// # }
    /// # }
    /// ```
    #[cfg(feature = "proto-2026-07-28-rc")]
    pub fn on_commit<F>(&self, effect: F)
    where
        F: std::future::Future<Output = Result<(), Error>> + Send + 'static,
    {
        match self.mrtr.as_ref() {
            Some(mrtr) => mrtr.push_commit(Box::pin(effect)),
            None => {
                #[cfg(feature = "tracing")]
                tracing::warn!(
                    logger = "neva",
                    "on_commit called outside an MRTR dispatch; ignored"
                );
            }
        }
    }

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
    #[cfg(all(feature = "tasks", not(feature = "proto-2026-07-28-rc")))]
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
    pub async fn complete_elicitation(&mut self, id: impl Into<String>) -> Result<(), Error> {
        let params = serde_json::to_value(ElicitationCompleteParams::new(id)).ok();
        self.send_notification(crate::types::elicitation::commands::COMPLETE, params)
            .await
    }

    /// Sends notification that a task with `id` was changed.
    #[cfg(feature = "tasks")]
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
    #[cfg(feature = "tasks")]
    #[cfg_attr(feature = "proto-2026-07-28-rc", allow(dead_code))]
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
    /// Server→client requests (non-RC elicitation/sampling/roots and the task
    /// API). The stateless RC transport has no out-of-band server→client
    /// channel, so this is unused there.
    #[inline]
    #[cfg_attr(feature = "proto-2026-07-28-rc", allow(dead_code))]
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
    /// Under the stateless `proto-2026-07-28-rc` transport there is no
    /// out-of-band server→client channel, so this is a no-op: progress,
    /// list-changed, resource-updated, task-status and elicitation
    /// notifications are inert and clients poll instead.
    #[inline]
    async fn send_notification(
        &mut self,
        #[cfg_attr(feature = "proto-2026-07-28-rc", allow(unused_variables))] method: &str,
        #[cfg_attr(feature = "proto-2026-07-28-rc", allow(unused_variables))] params: Option<
            serde_json::Value,
        >,
    ) -> Result<(), Error> {
        #[cfg(feature = "proto-2026-07-28-rc")]
        {
            Ok(())
        }
        #[cfg(not(feature = "proto-2026-07-28-rc"))]
        {
            let mut notification = Notification::new(method, params);
            if let Some(session_id) = self.session_id {
                notification.session_id = Some(session_id);
            }
            self.sender.send(notification.into()).await
        }
    }
}

#[cfg(feature = "tasks")]
impl crate::shared::TaskApi for Context {
    /// Retrieve task result from the client. If the task is not completed yet, waits until it completes or cancels.
    async fn get_task_result<T>(&mut self, id: impl Into<String>) -> Result<T, Error>
    where
        T: DeserializeOwned,
    {
        let params = GetTaskPayloadRequestParams { id: id.into() };
        let method = crate::types::task::commands::RESULT;
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(params),
        );

        self.send_request(req).await?.into_result()
    }

    /// Retrieve task status from the client
    async fn get_task(&mut self, id: impl Into<String>) -> Result<Task, Error> {
        let params = GetTaskRequestParams { id: id.into() };
        let method = crate::types::task::commands::GET;
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(params),
        );

        self.send_request(req).await?.into_result()
    }

    /// Cancels a task that is currently running on the client
    async fn cancel_task(&mut self, id: impl Into<String>) -> Result<Task, Error> {
        if !self.options.is_tasks_cancellation_supported() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "Server does not support cancelling tasks.",
            ));
        }

        let params = CancelTaskRequestParams { id: id.into() };
        let method = crate::types::task::commands::CANCEL;
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(params),
        );

        self.send_request(req).await?.into_result()
    }

    /// Retrieves a list of tasks from the client
    async fn list_tasks(&mut self, cursor: Option<Cursor>) -> Result<ListTasksResult, Error> {
        if !self.options.is_tasks_list_supported() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "Server does not support retrieving a task list.",
            ));
        }

        let params = ListTasksRequestParams { cursor };
        let method = crate::types::task::commands::LIST;
        let req = Request::new(
            Some(RequestId::Uuid(uuid::Uuid::new_v4())),
            method,
            Some(params),
        );

        self.send_request(req).await?.into_result()
    }

    async fn handle_input(&mut self, _id: &str, _params: TaskPayload) -> Result<(), Error> {
        // Reserved, there are no cases so far, for the server
        // to handle input requests from client.
        Ok(())
    }
}

#[cfg(test)]
#[cfg(feature = "server")]
mod missing_resource_error_tests {
    use crate::error::ErrorCode;

    #[test]
    fn missing_resource_uses_spec_version_code() {
        // The constant the emitters use must match the spec.
        #[cfg(feature = "proto-2026-07-28-rc")]
        assert_eq!(i32::from(ErrorCode::RESOURCE_NOT_FOUND), -32602);
        #[cfg(not(feature = "proto-2026-07-28-rc"))]
        assert_eq!(i32::from(ErrorCode::RESOURCE_NOT_FOUND), -32002);
    }
}

#[cfg(all(test, feature = "proto-2026-07-28-rc"))]
mod mrtr_tests {
    use super::*;
    use crate::types::elicitation::{ElicitRequestParams, ElicitResult, ElicitationAction};

    fn params() -> ElicitRequestParams {
        ElicitRequestParams::form("m")
            .with_required("x", "string")
            .into()
    }

    #[test]
    fn resolve_replays_cached_answer_and_records_pending_on_miss() {
        let mut answers = std::collections::HashMap::new();
        answers.insert(
            "known".to_string(),
            ElicitResult {
                action: ElicitationAction::Accept,
                content: Some(serde_json::json!({ "x": 1 })),
                meta: None,
            },
        );
        let mrtr = MrtrCtx {
            answers,
            pending: Default::default(),
            elicitation_allowed: true,
            ..Default::default()
        };

        // Hit: returns the cached answer.
        let got = mrtr.resolve("known".into(), params()).expect("cached");
        assert_eq!(got.action, ElicitationAction::Accept);

        // Miss: returns the sentinel and records pending.
        let miss = mrtr.resolve("unknown".into(), params());
        assert_eq!(miss.unwrap_err().code, ErrorCode::InputRequired);
        assert!(mrtr.pending.lock().unwrap().is_some());
    }

    #[test]
    fn effect_seen_and_record() {
        let m = MrtrCtx::default();
        assert!(!m.effect_seen("charge"));
        m.record_effect("charge".into());
        assert!(m.effect_seen("charge"));
    }

    #[test]
    fn cached_memo_store_and_fetch() {
        let m = MrtrCtx::default();
        assert!(m.cached_memo("quote").is_none());
        m.store_memo("quote".into(), serde_json::json!({"price": 42}));
        assert_eq!(
            m.cached_memo("quote"),
            Some(serde_json::json!({"price": 42}))
        );
    }

    #[test]
    fn push_commit_accumulates() {
        let m = MrtrCtx::default();
        m.push_commit(Box::pin(async { Ok(()) }));
        m.push_commit(Box::pin(async { Ok(()) }));
        assert_eq!(m.commits.lock().unwrap().len(), 2);
    }
}

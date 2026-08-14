//! MCP server options

use crate::app::{collection::Collection, handler::RequestHandler};
#[cfg(feature = "http-server")]
use crate::transport::{HttpEngine, HttpServer};
use crate::transport::{StdIoServer, TransportProto};
use dashmap::DashMap;
#[cfg(feature = "legacy-spec")]
use dashmap::DashSet;
use std::fmt::{Debug, Formatter};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

#[cfg(feature = "http-server-volga")]
use crate::transport::http::server::{DefaultClaims, VolgaEngine};

use crate::middleware::{Middleware, Middlewares};

use crate::PROTOCOL_VERSIONS;
use crate::types::{
    Cursor, Implementation, Prompt, PromptsCapability, ReadResourceResult, RequestId, Resource,
    ResourceTemplate, ResourcesCapability, Tool, ToolsCapability, Uri,
    resource::{Route, route::ResourceHandler},
};

#[cfg(feature = "tasks")]
use crate::shared::{TaskHandle, TaskTracker};
#[cfg(all(feature = "tasks", feature = "legacy-spec"))]
use crate::types::TaskPayload;
#[cfg(all(feature = "tracing", feature = "legacy-spec"))]
use crate::types::notification::LoggingLevel;
#[cfg(feature = "tasks")]
use crate::types::{ServerTasksCapability, Task};

#[cfg(all(feature = "tracing", feature = "legacy-spec"))]
use tracing_subscriber::{Registry, filter::LevelFilter, reload::Handle};

#[cfg(any(feature = "tasks", all(feature = "tracing", feature = "legacy-spec")))]
use crate::error::Error;
#[cfg(all(feature = "tracing", feature = "legacy-spec"))]
use crate::error::ErrorCode;

/// Represents MCP server options that are available in runtime
pub type RuntimeMcpOptions = Arc<McpOptions>;

/// Represents MCP server configuration options
pub struct McpOptions {
    /// Information of current server's implementation
    pub(crate) implementation: Implementation,

    /// Timeout for the requests from server to a client
    pub(crate) request_timeout: Duration,

    /// A map of tools, where the _key_ is a tool _name_
    pub(super) tools: Collection<Tool>,

    /// A map of prompts, where the _key_ is a prompt _name_
    pub(super) prompts: Collection<Prompt>,

    /// A map of resources, where the _key_ is a resource name
    pub(super) resources: Collection<Resource>,

    /// A flat map of resource templates, where the _key_ is a resource template name
    pub(super) resources_templates: Collection<ResourceTemplate>,

    /// Holds current subscriptions to resource changes
    #[cfg(feature = "legacy-spec")]
    pub(super) resource_subscriptions: DashSet<Uri>,

    /// Live `subscriptions/listen` streams (MCP 2026-07-28).
    ///
    /// Replaces [`Self::resource_subscriptions`]: a per-resource subscription
    /// is now a URI inside a listen filter, scoped to that stream's lifetime.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) subscriptions: crate::app::subscriptions::SubscriptionRegistry,

    /// Cancelled when the server shuts down, so long-lived requests
    /// (`subscriptions/listen`) can close gracefully instead of being dropped
    /// mid-stream.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) shutdown: CancellationToken,

    /// An ordered list of middlewares
    pub(super) middlewares: Option<Middlewares>,

    /// Tools capability options
    tools_capability: Option<ToolsCapability>,

    /// Resource capability options
    resources_capability: Option<ResourcesCapability>,

    /// Prompts capability options
    prompts_capability: Option<PromptsCapability>,

    /// Server tasks capability options
    #[cfg(feature = "tasks")]
    tasks_capability: Option<ServerTasksCapability>,

    /// Registered protocol extensions (MCP 2026-07-28), keyed by reverse-DNS
    /// id mapping to the extension's advertised capability value. Surfaced in
    /// `DiscoverResult` under `capabilities.extensions`.
    #[cfg(not(feature = "legacy-spec"))]
    extensions: std::collections::HashMap<String, serde_json::Value>,

    /// The last logging level set by the client
    #[cfg(all(feature = "tracing", feature = "legacy-spec"))]
    log_level: Option<Handle<LevelFilter, Registry>>,

    /// An MCP version that server supports
    protocol_ver: Option<&'static str>,

    /// Current transport protocol that this server uses
    proto: Option<TransportProto>,

    /// A resource template routing data structure
    resource_routes: Route,

    /// Currently running requests
    requests: DashMap<RequestId, CancellationToken>,

    /// Currently running tasks
    #[cfg(feature = "tasks")]
    pub(super) tasks: TaskTracker,

    /// Keyring used to encrypt and authenticate MRTR `requestState` (the AEAD
    /// key is derived from the secret the active kid names). Defaults to an
    /// ephemeral random single-key ring; multi-instance stateless deployments
    /// must set shared key material via
    /// [`crate::App::with_request_state_secret`] or
    /// [`crate::App::with_request_state_keys`].
    #[cfg(not(feature = "legacy-spec"))]
    request_state_keys: crate::types::mrtr::state::StateKeyring,

    /// Whether [`Self::request_state_keys`] was set explicitly (vs the
    /// ephemeral per-process default). Used to warn on startup about the
    /// multi-instance deployment footgun. Read only by the (tracing-gated)
    /// startup warning, so it is write-only in builds without an HTTP server
    /// or `tracing`.
    #[cfg(not(feature = "legacy-spec"))]
    #[cfg_attr(
        not(all(feature = "http-server", feature = "tracing")),
        allow(dead_code)
    )]
    request_state_secret_explicit: bool,

    /// TTL (seconds) embedded into MRTR `requestState`.
    #[cfg(not(feature = "legacy-spec"))]
    request_state_ttl_secs: u64,

    /// Service identity bound into MRTR `requestState`, set via
    /// [`crate::App::with_request_state_audience`]. `None` mints and demands
    /// an unbound state, which is what a deployment whose keyring is its own
    /// wants.
    #[cfg(not(feature = "legacy-spec"))]
    request_state_audience: Option<Box<str>>,

    /// Max encoded `requestState` blob length (bytes) before the server
    /// rejects the round-trip with "requestState too large".
    #[cfg(not(feature = "legacy-spec"))]
    max_state_bytes: usize,

    /// Store backing MRTR final-round idempotency. Defaults to a per-process
    /// in-memory cache; multi-instance deployments should set a shared store
    /// via [`crate::App::with_request_state_store`].
    #[cfg(not(feature = "legacy-spec"))]
    request_state_store: Arc<dyn crate::app::mrtr_store::RequestStateStore>,

    /// Carries subscribable notifications between instances of one logical
    /// server, set via [`crate::App::with_notification_bus`].
    ///
    /// `None` -- the default -- means notifications go straight to this
    /// instance's own [`Self::subscriptions`], which is all a single-instance
    /// server ever needs and costs nothing.
    #[cfg(not(feature = "legacy-spec"))]
    notification_bus: Option<Arc<dyn crate::app::notification_bus::DynNotificationBus>>,
}

impl Debug for McpOptions {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut binding = f.debug_struct("McpOptions");
        let dbg = binding
            .field("implementation", &self.implementation)
            .field("request_timeout", &self.request_timeout)
            .field("tools_capability", &self.tools_capability)
            .field("resources_capability", &self.resources_capability)
            .field("prompts_capability", &self.prompts_capability)
            .field("protocol_ver", &self.protocol_ver);

        #[cfg(feature = "tasks")]
        dbg.field("tasks_capability", &self.tasks_capability);

        #[cfg(all(feature = "tracing", feature = "legacy-spec"))]
        dbg.field("log_level", &self.log_level);

        dbg.finish()
    }
}

impl Default for McpOptions {
    #[inline]
    fn default() -> Self {
        Self {
            implementation: Default::default(),
            request_timeout: Duration::from_secs(10),
            tools: Collection::new(),
            resources: Collection::new(),
            prompts: Collection::new(),
            resources_templates: Collection::new(),
            proto: Default::default(),
            protocol_ver: Default::default(),
            tools_capability: Default::default(),
            resources_capability: Default::default(),
            prompts_capability: Default::default(),
            #[cfg(feature = "tasks")]
            tasks_capability: Default::default(),
            #[cfg(not(feature = "legacy-spec"))]
            extensions: Default::default(),
            resource_routes: Default::default(),
            requests: Default::default(),
            #[cfg(feature = "legacy-spec")]
            resource_subscriptions: Default::default(),
            #[cfg(not(feature = "legacy-spec"))]
            subscriptions: Default::default(),
            #[cfg(not(feature = "legacy-spec"))]
            shutdown: CancellationToken::new(),
            middlewares: None,
            #[cfg(all(feature = "tracing", feature = "legacy-spec"))]
            log_level: Default::default(),
            #[cfg(feature = "tasks")]
            tasks: TaskTracker::new(),
            #[cfg(not(feature = "legacy-spec"))]
            request_state_keys: {
                // Ephemeral random key from two v4 UUIDs (16 bytes each).
                // Non-panicking; sufficient for single-instance/dev.
                let mut key = [0u8; 32];
                key[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
                key[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
                crate::types::mrtr::state::StateKeyring::single(&key)
            },
            #[cfg(not(feature = "legacy-spec"))]
            request_state_secret_explicit: false,
            #[cfg(not(feature = "legacy-spec"))]
            request_state_ttl_secs: 300,
            #[cfg(not(feature = "legacy-spec"))]
            request_state_audience: None,
            #[cfg(not(feature = "legacy-spec"))]
            max_state_bytes: 8 * 1024,
            #[cfg(not(feature = "legacy-spec"))]
            request_state_store: Arc::new(crate::app::mrtr_store::InMemoryStateStore::new()),
            #[cfg(not(feature = "legacy-spec"))]
            notification_bus: None,
        }
    }
}

impl McpOptions {
    /// Sets stdio as a transport protocol
    pub fn with_stdio(mut self) -> Self {
        self.proto = Some(TransportProto::StdIoServer(StdIoServer::new()));
        self
    }

    /// Sets Streamable HTTP as a transport protocol.
    ///
    /// Accepts any `HttpServer<C, E>` for any engine `E: HttpEngine`. When
    /// no engine is specified (using the default `HttpServer::new(...)`),
    /// the Volga engine is used.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Default (Volga):
    /// let opts = McpOptions::default().set_http(HttpServer::new("127.0.0.1:3000"));
    /// ```
    #[cfg(feature = "http-server")]
    pub fn set_http<C, E>(mut self, http: HttpServer<C, E>) -> Self
    where
        C: Send + Sync + 'static,
        E: HttpEngine,
    {
        self.proto = Some(TransportProto::HttpServer(Box::new(http)));
        self
    }

    /// Sets Streamable HTTP as a transport protocol, using the default
    /// Volga engine. The closure receives the default-constructed server
    /// for fluent configuration.
    #[cfg(feature = "http-server-volga")]
    pub fn with_http<F>(mut self, config: F) -> Self
    where
        F: FnOnce(HttpServer<DefaultClaims, VolgaEngine>) -> HttpServer<DefaultClaims, VolgaEngine>,
    {
        self.proto = Some(TransportProto::HttpServer(Box::new(config(
            HttpServer::default(),
        ))));
        self
    }

    /// Sets Streamable HTTP as a transport protocol with default configuration
    ///
    /// Default:
    /// * __IP__: 127.0.0.1
    /// * __PORT__: 3000
    /// * __ENDPOINT__: /mcp
    #[cfg(feature = "http-server-volga")]
    pub fn with_default_http(self) -> Self {
        self.with_http(|http| http)
    }

    /// Specifies MCP server name
    pub fn with_name(mut self, name: &str) -> Self {
        self.implementation.name = name.into();
        self
    }

    /// Specifies the MCP server version
    pub fn with_version(mut self, ver: &str) -> Self {
        self.implementation.version = ver.into();
        self
    }

    /// Specifies Model Context Protocol version
    ///
    /// Default: last available legacy protocol version
    ///
    /// Available only under `legacy-spec`. The default build compiles the
    /// server as a pure MCP 2026-07-28 peer (sampling/roots removed, stateless
    /// transport, MRTR), so advertising an older version would claim a protocol
    /// the build cannot actually serve -- there the version is fixed at
    /// `2026-07-28`.
    #[cfg(feature = "legacy-spec")]
    pub fn with_mcp_version(mut self, ver: &'static str) -> Self {
        self.protocol_ver = Some(ver);
        self
    }

    /// Configures tools capability
    pub fn with_tools<F>(mut self, config: F) -> Self
    where
        F: FnOnce(ToolsCapability) -> ToolsCapability,
    {
        self.tools_capability = Some(config(Default::default()));
        self
    }

    /// Configures resources capability
    pub fn with_resources<F>(mut self, config: F) -> Self
    where
        F: FnOnce(ResourcesCapability) -> ResourcesCapability,
    {
        self.resources_capability = Some(config(Default::default()));
        self
    }

    /// Configures prompts capability
    pub fn with_prompts<F>(mut self, config: F) -> Self
    where
        F: FnOnce(PromptsCapability) -> PromptsCapability,
    {
        self.prompts_capability = Some(config(Default::default()));
        self
    }

    /// Configures tasks capability
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    pub fn with_tasks<F>(mut self, config: F) -> Self
    where
        F: FnOnce(ServerTasksCapability) -> ServerTasksCapability,
    {
        self.tasks_capability = Some(config(Default::default()));
        self
    }

    /// Enables the Tasks extension.
    ///
    /// Under MCP 2026-07-28 tasks are an extension whose capability carries no
    /// settings -- advertising it *is* the declaration -- so this takes no
    /// configuration. It registers through
    /// [`crate::app::extension::TasksExtension`], surfacing under
    /// `capabilities.extensions["io.modelcontextprotocol/tasks"]`.
    ///
    /// # Examples
    ///
    /// ```
    /// use neva::App;
    ///
    /// let app = App::new().with_options(|opt| opt.with_tasks());
    /// # let _ = app;
    /// ```
    #[cfg(all(feature = "tasks", not(feature = "legacy-spec")))]
    pub fn with_tasks(mut self) -> Self {
        use crate::app::extension::{Extension, TasksExtension};
        let capability = ServerTasksCapability::default();
        self.tasks_capability = Some(capability.clone());
        let ext = TasksExtension::new(capability);
        self.register_extension(ext.id(), ext.capability());
        self
    }

    /// Records an extension's advertised capability under its reverse-DNS id
    /// (MCP 2026-07-28). Used by [`crate::App::with_extension`] and by the
    /// `with_tasks` thin wrapper.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) fn register_extension(&mut self, id: &str, capability: serde_json::Value) {
        self.extensions.insert(id.into(), capability);
    }

    /// Sets the server tasks capability directly (used by the extension path).
    #[cfg(all(feature = "tasks", not(feature = "legacy-spec")))]
    pub(crate) fn set_tasks_capability(&mut self, capability: ServerTasksCapability) {
        self.tasks_capability = Some(capability);
    }

    /// Specifies request timeout
    ///
    /// Default: 10 seconds
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Configures a `tracing_subscriber::reload::Handle` that allows changing the [`LoggingLevel`] at runtime
    #[cfg_attr(
        feature = "legacy-spec",
        deprecated(
            note = "MCP server-side logging is removed in MCP 2026-07-28; this method will be removed when the legacy flag is dropped."
        )
    )]
    #[cfg(all(feature = "tracing", feature = "legacy-spec"))]
    pub fn with_logging(mut self, log_handle: Handle<LevelFilter, Registry>) -> Self {
        self.log_level = Some(log_handle);
        self
    }

    /// Sets the [`LoggingLevel`]
    #[cfg_attr(
        feature = "legacy-spec",
        deprecated(
            note = "MCP server-side logging is removed in MCP 2026-07-28; this method will be removed when the legacy flag is dropped."
        )
    )]
    #[cfg(all(feature = "tracing", feature = "legacy-spec"))]
    pub fn set_log_level(&self, level: LoggingLevel) -> Result<(), Error> {
        if let Some(handle) = &self.log_level {
            handle
                .modify(|current| *current = level.into())
                .map_err(|e| Error::new(ErrorCode::InternalError, e.to_string()))?;
        }
        Ok(())
    }

    /// Returns current log level
    #[cfg(all(feature = "tracing", feature = "legacy-spec"))]
    pub(crate) fn log_level(&self) -> Option<LoggingLevel> {
        match &self.log_level {
            None => None,
            Some(handle) => handle.clone_current().map(|x| x.into()),
        }
    }

    /// Tracks the request with `req_id` and returns the [`CancellationToken`] for this request
    pub(crate) fn track_request(&self, req_id: &RequestId) -> CancellationToken {
        let token = CancellationToken::new();
        self.requests.insert(req_id.clone(), token.clone());
        token
    }

    /// Cancels the request with `req_id` if it is present
    pub(crate) fn cancel_request(&self, req_id: &RequestId) {
        if let Some((_, token)) = self.requests.remove(req_id) {
            token.cancel();
        }
    }

    /// Completes the request with `req_id` if it is present
    pub(crate) fn complete_request(&self, req_id: &RequestId) {
        self.requests.remove(req_id);
    }

    /// Returns a list of currently running tasks
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    pub(crate) fn list_tasks(&self) -> Vec<Task> {
        self.tasks.tasks()
    }

    /// Tacks the task and returns the [`CancellationToken`] for this task
    #[cfg(feature = "tasks")]
    pub(crate) fn track_task(&self, task: Task) -> TaskHandle {
        self.tasks.track(task)
    }

    /// Cancels the task
    #[cfg(feature = "tasks")]
    pub(crate) fn cancel_task(&self, task_id: &str) -> Result<Task, Error> {
        self.tasks.cancel(task_id)
    }

    /// Retrieves the task status
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    pub(crate) fn get_task_status(&self, task_id: &str) -> Result<Task, Error> {
        self.tasks.get_status(task_id)
    }

    /// Retrieves the full task state served by `tasks/get` (MCP 2026-07-28)
    #[cfg(all(feature = "tasks", not(feature = "legacy-spec")))]
    pub(crate) fn get_task_state(
        &self,
        task_id: &str,
    ) -> Result<crate::types::DetailedTask, Error> {
        self.tasks.get_state(task_id)
    }

    /// Delivers client answers to a task's outstanding input requests
    /// (`tasks/update`, MCP 2026-07-28)
    #[cfg(all(feature = "tasks", not(feature = "legacy-spec")))]
    pub(crate) fn update_task(
        &self,
        task_id: &str,
        responses: crate::types::mrtr::InputResponses,
    ) -> Result<(), Error> {
        self.tasks.provide_inputs(task_id, responses)
    }

    /// Awaits the task result
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    pub(crate) async fn get_task_result(&self, task_id: &str) -> Result<TaskPayload, Error> {
        self.tasks.get_result(task_id).await
    }

    /// Adds a tool
    pub(crate) fn add_tool(&mut self, tool: Tool) -> &mut Tool {
        self.tools_capability.get_or_insert_default();

        self.tools.as_mut().entry(tool.name.clone()).or_insert(tool)
    }

    /// Adds a resource
    pub(crate) fn add_resource(&mut self, resource: Resource) -> &mut Resource {
        self.resources_capability.get_or_insert_default();

        self.resources
            .as_mut()
            .entry(resource.uri.to_string())
            .or_insert(resource)
    }

    /// Adds a resource template
    pub(crate) fn add_resource_template(
        &mut self,
        template: ResourceTemplate,
        handler: RequestHandler<ReadResourceResult>,
    ) -> &mut ResourceTemplate {
        self.resources_capability.get_or_insert_default();

        let name = template.name.clone();

        self.resource_routes
            .insert(&template.uri_template, name.clone(), handler);
        self.resources_templates
            .as_mut()
            .entry(name)
            .or_insert(template)
    }

    /// Adds a prompt
    pub(crate) fn add_prompt(&mut self, prompt: Prompt) -> &mut Prompt {
        self.prompts_capability.get_or_insert_default();

        self.prompts
            .as_mut()
            .entry(prompt.name.clone())
            .or_insert(prompt)
    }

    /// Registers a middleware
    #[inline]
    pub(crate) fn add_middleware(&mut self, middleware: Middleware) {
        self.middlewares
            .get_or_insert_with(Middlewares::new)
            .add(middleware);
    }

    /// Registers a middleware as the outermost layer of the pipeline.
    #[inline]
    #[cfg(feature = "tracing")]
    pub(crate) fn add_middleware_front(&mut self, middleware: Middleware) {
        self.middlewares
            .get_or_insert_with(Middlewares::new)
            .add_front(middleware);
    }

    /// Returns a Model Context Protocol version that this server supports
    #[inline]
    pub(crate) fn protocol_ver(&self) -> &'static str {
        match self.protocol_ver {
            Some(ver) => ver,
            None => PROTOCOL_VERSIONS.last().unwrap(),
        }
    }

    /// Returns current transport protocol
    pub(crate) fn transport(&mut self) -> TransportProto {
        let transport = self.proto.take();
        transport.unwrap_or_default()
    }

    /// Returns a display label for the currently configured transport
    pub(super) fn transport_label(&self) -> String {
        match &self.proto {
            Some(TransportProto::StdIoServer(_)) => "stdio".to_owned(),
            #[cfg(feature = "http-server")]
            Some(TransportProto::HttpServer(http)) => http.url_label(),
            _ => "(none)".to_owned(),
        }
    }

    /// Returns a tool by its name
    #[inline]
    pub(crate) async fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.get(name).await
    }

    /// Returns a paginated list of available tools.
    #[inline]
    pub(crate) async fn list_tools_page(
        &self,
        cursor: Option<Cursor>,
        page_size: usize,
    ) -> (Vec<Tool>, Option<Cursor>) {
        self.tools.page_values(cursor, page_size).await
    }

    /// Reads a resource by its URI
    #[inline]
    pub(crate) fn read_resource(&self, uri: &Uri) -> Option<(&ResourceHandler, Box<[String]>)> {
        self.resource_routes.find(uri)
    }

    /// Returns a paginated list of available resources.
    #[inline]
    pub(crate) async fn list_resources_page(
        &self,
        cursor: Option<Cursor>,
        page_size: usize,
    ) -> (Vec<Resource>, Option<Cursor>) {
        self.resources.page_values(cursor, page_size).await
    }

    /// Returns a paginated list of available resource templates.
    #[inline]
    pub(crate) async fn list_resource_templates_page(
        &self,
        cursor: Option<Cursor>,
        page_size: usize,
    ) -> (Vec<ResourceTemplate>, Option<Cursor>) {
        self.resources_templates
            .page_values(cursor, page_size)
            .await
    }

    /// Returns a tool by its name
    #[inline]
    pub(crate) async fn get_prompt(&self, name: &str) -> Option<Prompt> {
        self.prompts.get(name).await
    }

    /// Returns a paginated list of available prompts.
    #[inline]
    pub(crate) async fn list_prompts_page(
        &self,
        cursor: Option<Cursor>,
        page_size: usize,
    ) -> (Vec<Prompt>, Option<Cursor>) {
        self.prompts.page_values(cursor, page_size).await
    }

    /// Returns [`ToolsCapability`] if configured.
    /// If not configured but at least one [`Tool`] exists, returns [`Default`].
    /// Otherwise, returns `None`.
    pub(crate) fn tools_capability(&self) -> Option<ToolsCapability> {
        self.tools_capability.clone()
    }

    /// Returns [`ResourcesCapability`] if configured.
    /// If not configured but at least one [`Resource`] or [`ResourceTemplate`] exists, returns [`Default`].
    /// Otherwise, returns `None`.
    pub(crate) fn resources_capability(&self) -> Option<ResourcesCapability> {
        self.resources_capability.clone()
    }

    /// Returns [`PromptsCapability`] if configured.
    /// If not configured but at least one [`Prompt`] exists, returns [`Default`].
    /// Otherwise, returns `None`.
    pub(crate) fn prompts_capability(&self) -> Option<PromptsCapability> {
        self.prompts_capability.clone()
    }

    /// Returns the capabilities this server advertises, which is what a
    /// `subscriptions/listen` filter is narrowed against: a client cannot
    /// subscribe to a notification type the server never announced.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) fn advertised_capabilities(&self) -> crate::types::ServerCapabilities {
        crate::types::ServerCapabilities {
            tools: self.tools_capability(),
            resources: self.resources_capability(),
            prompts: self.prompts_capability(),
            ..Default::default()
        }
    }

    /// Returns the registry of live `subscriptions/listen` streams.
    #[cfg(not(feature = "legacy-spec"))]
    #[inline]
    pub(crate) fn subscriptions(&self) -> &crate::app::subscriptions::SubscriptionRegistry {
        &self.subscriptions
    }

    /// Returns the token cancelled when the server shuts down.
    #[cfg(not(feature = "legacy-spec"))]
    #[inline]
    pub(crate) fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    /// Points the server's shutdown token at the transport's, so long-lived
    /// requests observe the same signal the dispatch loop does.
    #[cfg(not(feature = "legacy-spec"))]
    #[inline]
    pub(crate) fn set_shutdown_token(&mut self, token: CancellationToken) {
        self.shutdown = token;
    }

    /// Returns [`ServerTasksCapability`] if configured.
    ///
    /// Otherwise, returns `None`.
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    pub(crate) fn tasks_capability(&self) -> Option<ServerTasksCapability> {
        self.tasks_capability.clone()
    }

    /// Returns the registered protocol extensions as a capability map
    /// (MCP 2026-07-28), or `None` when no extension is registered so the
    /// `capabilities.extensions` field is omitted on the wire.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) fn extensions(
        &self,
    ) -> Option<std::collections::HashMap<String, serde_json::Value>> {
        if self.extensions.is_empty() {
            None
        } else {
            Some(self.extensions.clone())
        }
    }

    /// Returns whether the server is configured to send the "notifications/resources/updated"
    #[inline]
    pub(crate) fn is_resource_subscription_supported(&self) -> bool {
        self.resources_capability
            .as_ref()
            .is_some_and(|res| res.subscribe)
    }

    /// Returns whether the server is configured to send the "notifications/resources/list_changed"
    #[inline]
    pub(crate) fn is_resource_list_changed_supported(&self) -> bool {
        self.resources_capability
            .as_ref()
            .is_some_and(|res| res.list_changed)
    }

    /// Returns whether the server is configured to send the "notifications/tools/list_changed"
    #[inline]
    pub(crate) fn is_tools_list_changed_supported(&self) -> bool {
        self.tools_capability
            .as_ref()
            .is_some_and(|tool| tool.list_changed)
    }

    /// Returns whether the server is configured to send the "notifications/prompts/list_changed"
    #[inline]
    pub(crate) fn is_prompts_list_changed_supported(&self) -> bool {
        self.prompts_capability
            .as_ref()
            .is_some_and(|prompt| prompt.list_changed)
    }

    /// Returns whether the server is configured to handle the "tasks/list" requests.
    #[inline]
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    pub(crate) fn is_tasks_list_supported(&self) -> bool {
        self.tasks_capability
            .as_ref()
            .is_some_and(|tasks| tasks.list.is_some())
    }

    /// Returns whether the server is configured to handle the "tasks/cancel" requests.
    #[inline]
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    pub(crate) fn is_tasks_cancellation_supported(&self) -> bool {
        self.tasks_capability
            .as_ref()
            .is_some_and(|tasks| tasks.cancel.is_some())
    }

    /// Returns whether the server is configured to handle the task-augmented "tools/call" requests.
    ///
    /// Under MCP 2026-07-28 the Tasks extension capability carries no
    /// per-request settings: advertising the extension at all is the
    /// declaration, and the server decides per request whether to defer.
    #[inline]
    #[cfg(all(feature = "tasks", not(feature = "legacy-spec")))]
    pub(crate) fn is_task_augmented_tool_call_supported(&self) -> bool {
        self.tasks_capability.is_some()
    }

    /// Returns whether the server is configured to handle the task-augmented "tools/call" requests.
    #[inline]
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    pub(crate) fn is_task_augmented_tool_call_supported(&self) -> bool {
        self.tasks_capability
            .as_ref()
            .and_then(|tasks| tasks.requests.as_ref())
            .and_then(|req| req.tools.as_ref())
            .is_some_and(|tools| tools.call.is_some())
    }

    /// Sets the shared secret used to encrypt/authenticate MRTR `requestState`
    /// as a single-key ring under the default kid.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) fn set_request_state_secret(&mut self, key: &[u8]) {
        self.request_state_keys = crate::types::mrtr::state::StateKeyring::single(key);
        self.request_state_secret_explicit = true;
    }

    /// Sets the MRTR `requestState` keyring: new blobs are sealed under
    /// `active_kid`, inbound blobs decrypt with whichever accepted key their
    /// kid segment names.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) fn set_request_state_keys<K, S>(
        &mut self,
        active_kid: &str,
        keys: impl IntoIterator<Item = (K, S)>,
    ) where
        K: AsRef<str>,
        S: AsRef<[u8]>,
    {
        self.request_state_keys = crate::types::mrtr::state::StateKeyring::new(active_kid, keys);
        self.request_state_secret_explicit = true;
    }

    /// Returns the MRTR `requestState` keyring (AEAD key material).
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) fn request_state_keys(&self) -> &crate::types::mrtr::state::StateKeyring {
        &self.request_state_keys
    }

    /// Returns whether the MRTR `requestState` secret was set explicitly
    /// (vs the ephemeral per-process default).
    ///
    /// Only compiled with `tracing`, where it backs the startup deployment
    /// warning in [`crate::App::run`]; without it the field has no reader.
    #[cfg(all(
        not(feature = "legacy-spec"),
        feature = "http-server",
        feature = "tracing"
    ))]
    pub(crate) fn request_state_secret_is_explicit(&self) -> bool {
        self.request_state_secret_explicit
    }

    /// Returns whether the configured transport is the HTTP server transport.
    #[cfg(all(
        not(feature = "legacy-spec"),
        feature = "http-server",
        feature = "tracing"
    ))]
    pub(crate) fn is_http_transport(&self) -> bool {
        matches!(self.proto, Some(TransportProto::HttpServer(_)))
    }

    /// Returns the MRTR `requestState` TTL in seconds.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) fn request_state_ttl_secs(&self) -> u64 {
        self.request_state_ttl_secs
    }

    /// Sets the service identity bound into MRTR `requestState`.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) fn set_request_state_audience(&mut self, audience: &str) {
        self.request_state_audience = Some(Box::from(audience));
    }

    /// Returns the service identity bound into MRTR `requestState`.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) fn request_state_audience(&self) -> Option<&str> {
        self.request_state_audience.as_deref()
    }

    /// Sets the max encoded `requestState` size in bytes.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) fn set_max_state_bytes(&mut self, bytes: usize) {
        self.max_state_bytes = bytes;
    }

    /// Returns the max encoded `requestState` size in bytes.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) fn max_state_bytes(&self) -> usize {
        self.max_state_bytes
    }

    /// Sets the MRTR final-round idempotency store.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) fn set_request_state_store(
        &mut self,
        store: Arc<dyn crate::app::mrtr_store::RequestStateStore>,
    ) {
        self.request_state_store = store;
    }

    /// Returns the MRTR final-round idempotency store.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) fn request_state_store(&self) -> &dyn crate::app::mrtr_store::RequestStateStore {
        self.request_state_store.as_ref()
    }

    /// Sets the bus carrying subscription notifications between instances.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) fn set_notification_bus(
        &mut self,
        bus: Arc<dyn crate::app::notification_bus::DynNotificationBus>,
    ) {
        self.notification_bus = Some(bus);
    }

    /// Returns the bus carrying subscription notifications between instances,
    /// or `None` when notifications are delivered to local subscribers only.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) fn notification_bus(
        &self,
    ) -> Option<&Arc<dyn crate::app::notification_bus::DynNotificationBus>> {
        self.notification_bus.as_ref()
    }

    /// Turns [`McpOptions`] into [`RuntimeMcpOptions`]
    pub(crate) fn into_runtime(mut self) -> RuntimeMcpOptions {
        self.tools = self.tools.into_runtime();
        self.prompts = self.prompts.into_runtime();
        self.resources = self.resources.into_runtime();
        self.resources_templates = self.resources_templates.into_runtime();
        Arc::new(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SDK_NAME;
    use crate::error::{Error, ErrorCode};
    use crate::types::resource::Uri;
    use crate::types::resource::template::ResourceFunc;
    use crate::types::{
        GetPromptRequestParams, PromptMessage, ReadResourceRequestParams, ResourceContents, Role,
    };

    #[test]
    fn it_creates_default_options() {
        let options = McpOptions::default();

        assert_eq!(options.implementation.name, SDK_NAME);
        assert_eq!(options.implementation.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(options.tools.as_ref().len(), 0);
        assert_eq!(options.resources.as_ref().len(), 0);
        assert_eq!(options.resources_templates.as_ref().len(), 0);
        assert_eq!(options.prompts.as_ref().len(), 0);
        assert!(options.proto.is_none());
    }

    #[test]
    fn it_takes_none_transport_by_default() {
        let mut options = McpOptions::default();

        let transport = options.transport();

        assert!(matches!(transport, TransportProto::None));
    }

    #[test]
    fn it_sets_and_takes_stdio_transport() {
        let mut options = McpOptions::default().with_stdio();

        let transport = options.transport();

        assert!(matches!(transport, TransportProto::StdIoServer(_)));
    }

    #[test]
    fn it_sets_server_name() {
        let options = McpOptions::default().with_name("name");

        assert_eq!(options.implementation.name, "name");
    }

    #[test]
    fn it_sets_server_version() {
        let options = McpOptions::default().with_version("1");

        assert_eq!(options.implementation.version, "1");
    }

    #[tokio::test]
    async fn it_adds_and_gets_tool() {
        let mut options = McpOptions::default();

        options.add_tool(Tool::new("tool", || async { "test" }));

        let tool = options.get_tool("tool").await.unwrap();
        assert_eq!(tool.name, "tool");
    }

    #[tokio::test]
    async fn it_returns_tools() {
        let mut options = McpOptions::default();

        options.add_tool(Tool::new("tool", || async { "test" }));

        let (tools, next_cursor) = options.list_tools_page(None, 10).await;
        assert_eq!(tools.len(), 1);
        assert_eq!(next_cursor, None);
    }

    #[tokio::test]
    async fn it_returns_resources() {
        let mut options = McpOptions::default();

        options.add_resource(Resource::new("res://res", "res"));

        let (resources, next_cursor) = options.list_resources_page(None, 10).await;
        assert_eq!(resources.len(), 1);
        assert_eq!(next_cursor, None);
    }

    #[tokio::test]
    async fn it_adds_and_reads_resource_template() {
        let mut options = McpOptions::default();

        let handler = |uri: Uri| async move {
            ResourceContents::new(uri)
                .with_mime("text/plain")
                .with_text("some text")
        };

        options.add_resource_template(
            ResourceTemplate::new("res://res", "test"),
            ResourceFunc::new(handler),
        );

        let req = ReadResourceRequestParams {
            uri: "res://res".into(),
            meta: None,
            args: None,
        };

        let res = options.read_resource(&req.uri).unwrap();
        let res = res.0.call(req.into()).await.unwrap();
        assert_eq!(res.contents.len(), 1);
    }

    #[tokio::test]
    async fn it_adds_and_reads_resource_template_with_err() {
        let mut options = McpOptions::default();

        let handler = |_: Uri| async move {
            Err::<ResourceContents, _>(Error::from(ErrorCode::RESOURCE_NOT_FOUND))
        };

        options.add_resource_template(
            ResourceTemplate::new("res://res", "test"),
            ResourceFunc::new(handler),
        );

        let req = ReadResourceRequestParams {
            uri: "res://res".into(),
            meta: None,
            args: None,
        };

        let res = options.read_resource(&req.uri).unwrap();
        let res = res.0.call(req.into()).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn it_returns_resource_templates() {
        let mut options = McpOptions::default();

        let handler = |uri: Uri| async move {
            ResourceContents::new(uri)
                .with_mime("text/plain")
                .with_text("some text")
        };

        options.add_resource_template(
            ResourceTemplate::new("res://res", "test"),
            ResourceFunc::new(handler),
        );

        let (resources, next_cursor) = options.list_resource_templates_page(None, 10).await;
        assert_eq!(resources.len(), 1);
        assert_eq!(next_cursor, None);
    }

    #[tokio::test]
    async fn it_adds_and_gets_prompt() {
        let mut options = McpOptions::default();

        options.add_prompt(Prompt::new("test", || async { [("test", Role::User)] }));

        let prompt = options.get_prompt("test").await.unwrap();
        assert_eq!(prompt.name, "test");

        let req = GetPromptRequestParams {
            name: "test".into(),
            args: None,
            meta: None,
        };

        let result = prompt.call(req).await.unwrap();

        let msg = result.messages.first().unwrap();

        assert_eq!(msg.role, Role::User)
    }

    #[tokio::test]
    async fn it_adds_and_gets_prompt_with_error() {
        let mut options = McpOptions::default();

        options.add_prompt(Prompt::new("test", || async {
            Err::<PromptMessage, _>(Error::from(ErrorCode::InternalError))
        }));

        let prompt = options.get_prompt("test").await.unwrap();
        assert_eq!(prompt.name, "test");

        let req = GetPromptRequestParams {
            name: "test".into(),
            args: None,
            meta: None,
        };

        let result = prompt.call(req).await;

        assert!(result.is_err())
    }

    #[tokio::test]
    async fn it_returns_prompts() {
        let mut options = McpOptions::default();

        options.add_prompt(Prompt::new("test", || async { [("test", Role::User)] }));

        let (prompts, next_cursor) = options.list_prompts_page(None, 10).await;
        assert_eq!(prompts.len(), 1);
        assert_eq!(next_cursor, None);
    }

    #[test]
    fn it_returns_some_tool_capabilities_if_configured() {
        let options = McpOptions::default().with_tools(|tools| tools.with_list_changed());

        let tools_capability = options.tools_capability().unwrap();

        // `listChanged` round-trips the config in both profiles: under MCP
        // 2026-07-28 a `subscriptions/listen` stream delivers it, so the
        // capability is honest again.
        assert!(tools_capability.list_changed);
    }

    #[test]
    fn it_returns_some_tool_capabilities_if_there_are_tools() {
        let mut options = McpOptions::default();
        options.add_tool(Tool::new("tool", || async { "test" }));

        let tools_capability = options.tools_capability().unwrap();

        assert!(!tools_capability.list_changed);
    }

    #[test]
    fn it_returns_none_tool_capabilities() {
        let options = McpOptions::default();

        assert!(options.tools_capability().is_none());
    }

    #[test]
    fn it_returns_some_resource_capabilities_if_configured() {
        let options = McpOptions::default().with_resources(|res| res.with_list_changed());

        let resources_capability = options.resources_capability().unwrap();

        assert!(resources_capability.list_changed);
    }

    #[test]
    fn it_returns_some_resources_capability_if_there_are_resources() {
        let mut options = McpOptions::default();
        options.add_resource(Resource::new("res", "test"));

        let resources_capability = options.resources_capability().unwrap();

        assert!(!resources_capability.list_changed);
    }

    #[test]
    fn it_returns_some_resources_capability_if_there_are_resource_templates() {
        let mut options = McpOptions::default();

        let handler = |_: Uri| async move {
            Err::<ResourceContents, _>(Error::from(ErrorCode::RESOURCE_NOT_FOUND))
        };

        options.add_resource_template(
            ResourceTemplate::new("res://test", "test"),
            ResourceFunc::new(handler),
        );

        let resources_capability = options.resources_capability().unwrap();

        assert!(!resources_capability.list_changed);
    }

    #[test]
    fn it_returns_none_resources_capability() {
        let options = McpOptions::default();

        assert!(options.resources_capability().is_none());
    }

    #[cfg(all(
        not(feature = "legacy-spec"),
        feature = "http-server",
        feature = "tracing"
    ))]
    #[test]
    fn request_state_secret_is_not_explicit_by_default() {
        let options = McpOptions::default();
        assert!(!options.request_state_secret_is_explicit());
    }

    #[cfg(all(
        not(feature = "legacy-spec"),
        feature = "http-server",
        feature = "tracing"
    ))]
    #[test]
    fn request_state_secret_is_explicit_once_set() {
        let mut options = McpOptions::default();
        options.set_request_state_secret(b"shared-secret");
        assert!(options.request_state_secret_is_explicit());
    }

    #[cfg(all(
        not(feature = "legacy-spec"),
        feature = "http-server",
        feature = "tracing"
    ))]
    #[test]
    fn request_state_keys_are_explicit_once_set() {
        let mut options = McpOptions::default();
        options.set_request_state_keys("1", [("1", b"shared-secret")]);
        assert!(options.request_state_secret_is_explicit());
    }

    #[test]
    fn it_returns_some_prompts_capability_if_configured() {
        let options = McpOptions::default().with_prompts(|prompts| prompts.with_list_changed());

        let prompts_capability = options.prompts_capability().unwrap();

        assert!(prompts_capability.list_changed);
    }

    #[test]
    fn it_returns_some_prompts_capability_if_there_are_tools() {
        let mut options = McpOptions::default();
        options.add_prompt(Prompt::new("test", || async {
            Err::<PromptMessage, _>(Error::from(ErrorCode::InternalError))
        }));

        let prompts_capability = options.prompts_capability().unwrap();

        assert!(!prompts_capability.list_changed);
    }

    #[test]
    fn it_returns_none_prompts_capability() {
        let options = McpOptions::default();

        assert!(options.prompts_capability().is_none());
    }

    #[test]
    fn it_returns_stdio_label() {
        let options = McpOptions::default().with_stdio();
        assert_eq!(options.transport_label(), "stdio");
    }

    #[test]
    fn it_returns_none_label_when_no_transport() {
        let options = McpOptions::default();
        assert_eq!(options.transport_label(), "(none)");
    }

    #[cfg(feature = "http-server-volga")]
    #[test]
    fn it_returns_http_label_when_http_transport() {
        let options = McpOptions::default().with_default_http();
        // Default HTTP: 127.0.0.1:3000/mcp
        assert_eq!(options.transport_label(), "http://127.0.0.1:3000/mcp");
    }
}

//! MCP server options

use crate::app::{collection::Collection, handler::RequestHandler};
#[cfg(feature = "http-server")]
use crate::transport::{HttpEngine, HttpServer};
use crate::transport::{StdIoServer, TransportProto};
use dashmap::{DashMap, DashSet};
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
#[cfg(all(feature = "tracing", not(feature = "proto-2026-07-28-rc")))]
use crate::types::notification::LoggingLevel;
#[cfg(feature = "tasks")]
use crate::types::{ServerTasksCapability, Task, TaskPayload};

#[cfg(all(feature = "tracing", not(feature = "proto-2026-07-28-rc")))]
use tracing_subscriber::{Registry, filter::LevelFilter, reload::Handle};

#[cfg(any(
    feature = "tasks",
    all(feature = "tracing", not(feature = "proto-2026-07-28-rc"))
))]
use crate::error::Error;
#[cfg(all(feature = "tracing", not(feature = "proto-2026-07-28-rc")))]
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
    pub(super) resource_subscriptions: DashSet<Uri>,

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

    /// Registered protocol extensions (MCP 2026-07-28 RC), keyed by reverse-DNS
    /// id mapping to the extension's advertised capability value. Surfaced in
    /// `DiscoverResult` under `capabilities.extensions`.
    #[cfg(feature = "proto-2026-07-28-rc")]
    extensions: std::collections::HashMap<String, serde_json::Value>,

    /// The last logging level set by the client
    #[cfg(all(feature = "tracing", not(feature = "proto-2026-07-28-rc")))]
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

    /// HMAC key signing MRTR `requestState`. Defaults to an ephemeral random
    /// key; multi-instance stateless deployments must set a shared secret via
    /// [`crate::App::with_request_state_secret`].
    #[cfg(feature = "proto-2026-07-28-rc")]
    request_state_secret: Arc<[u8]>,

    /// Whether [`Self::request_state_secret`] was set explicitly (vs the
    /// ephemeral per-process default). Used to warn on startup about the
    /// multi-instance deployment footgun. Read only by the (tracing-gated)
    /// startup warning, so it is write-only in builds without an HTTP server
    /// or `tracing`.
    #[cfg(feature = "proto-2026-07-28-rc")]
    #[cfg_attr(
        not(all(feature = "http-server", feature = "tracing")),
        allow(dead_code)
    )]
    request_state_secret_explicit: bool,

    /// TTL (seconds) embedded into MRTR `requestState`.
    #[cfg(feature = "proto-2026-07-28-rc")]
    request_state_ttl_secs: u64,

    /// Max encoded `requestState` blob length (bytes) before the server
    /// rejects the round-trip with "requestState too large".
    #[cfg(feature = "proto-2026-07-28-rc")]
    max_state_bytes: usize,

    /// Store backing MRTR final-round idempotency. Defaults to a per-process
    /// in-memory cache; multi-instance deployments should set a shared store
    /// via [`crate::App::with_request_state_store`].
    #[cfg(feature = "proto-2026-07-28-rc")]
    request_state_store: Arc<dyn crate::app::mrtr_store::RequestStateStore>,
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

        #[cfg(all(feature = "tracing", not(feature = "proto-2026-07-28-rc")))]
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
            #[cfg(feature = "proto-2026-07-28-rc")]
            extensions: Default::default(),
            resource_routes: Default::default(),
            requests: Default::default(),
            resource_subscriptions: Default::default(),
            middlewares: None,
            #[cfg(all(feature = "tracing", not(feature = "proto-2026-07-28-rc")))]
            log_level: Default::default(),
            #[cfg(feature = "tasks")]
            tasks: TaskTracker::new(),
            #[cfg(feature = "proto-2026-07-28-rc")]
            request_state_secret: {
                // Ephemeral random key from two v4 UUIDs (16 bytes each).
                // Non-panicking; sufficient for single-instance/dev.
                let mut key = [0u8; 32];
                key[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
                key[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
                Arc::from(&key[..])
            },
            #[cfg(feature = "proto-2026-07-28-rc")]
            request_state_secret_explicit: false,
            #[cfg(feature = "proto-2026-07-28-rc")]
            request_state_ttl_secs: 300,
            #[cfg(feature = "proto-2026-07-28-rc")]
            max_state_bytes: 8 * 1024,
            #[cfg(feature = "proto-2026-07-28-rc")]
            request_state_store: Arc::new(crate::app::mrtr_store::InMemoryStateStore::new()),
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
    /// Default: last available protocol version
    ///
    /// Not available under `proto-2026-07-28-rc`: that flag compiles the server
    /// as a pure 2026-07-28 RC peer (sampling/roots removed, stateless transport,
    /// MRTR), so advertising an older version would claim a protocol the build
    /// cannot actually serve. The RC version is fixed. When the RC graduates and
    /// the flags invert, version selection returns under the legacy flag.
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
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
    #[cfg(all(feature = "tasks", not(feature = "proto-2026-07-28-rc")))]
    pub fn with_tasks<F>(mut self, config: F) -> Self
    where
        F: FnOnce(ServerTasksCapability) -> ServerTasksCapability,
    {
        self.tasks_capability = Some(config(Default::default()));
        self
    }

    /// Configures tasks capability.
    ///
    /// Under `proto-2026-07-28-rc` tasks are an extension: this thin wrapper
    /// keeps the existing ergonomics while registering the capability through
    /// [`crate::app::extension::TasksExtension`] so it surfaces under
    /// `capabilities.extensions["io.modelcontextprotocol/tasks"]`.
    #[cfg(all(feature = "tasks", feature = "proto-2026-07-28-rc"))]
    pub fn with_tasks<F>(mut self, config: F) -> Self
    where
        F: FnOnce(ServerTasksCapability) -> ServerTasksCapability,
    {
        use crate::app::extension::{Extension, TasksExtension};
        let capability = config(Default::default());
        self.tasks_capability = Some(capability.clone());
        let ext = TasksExtension::new(capability);
        self.register_extension(ext.id(), ext.capability());
        self
    }

    /// Records an extension's advertised capability under its reverse-DNS id
    /// (MCP 2026-07-28 RC). Used by [`crate::App::with_extension`] and by the
    /// `with_tasks` thin wrapper.
    #[cfg(feature = "proto-2026-07-28-rc")]
    pub(crate) fn register_extension(&mut self, id: &str, capability: serde_json::Value) {
        self.extensions.insert(id.into(), capability);
    }

    /// Sets the server tasks capability directly (used by the extension path).
    #[cfg(all(feature = "tasks", feature = "proto-2026-07-28-rc"))]
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
        not(feature = "proto-2026-07-28-rc"),
        deprecated(
            note = "MCP server-side logging is removed in MCP 2026-07-28; this method will be removed when the legacy flag is dropped."
        )
    )]
    #[cfg(all(feature = "tracing", not(feature = "proto-2026-07-28-rc")))]
    pub fn with_logging(mut self, log_handle: Handle<LevelFilter, Registry>) -> Self {
        self.log_level = Some(log_handle);
        self
    }

    /// Sets the [`LoggingLevel`]
    #[cfg_attr(
        not(feature = "proto-2026-07-28-rc"),
        deprecated(
            note = "MCP server-side logging is removed in MCP 2026-07-28; this method will be removed when the legacy flag is dropped."
        )
    )]
    #[cfg(all(feature = "tracing", not(feature = "proto-2026-07-28-rc")))]
    pub fn set_log_level(&self, level: LoggingLevel) -> Result<(), Error> {
        if let Some(handle) = &self.log_level {
            handle
                .modify(|current| *current = level.into())
                .map_err(|e| Error::new(ErrorCode::InternalError, e.to_string()))?;
        }
        Ok(())
    }

    /// Returns current log level
    #[cfg(all(feature = "tracing", not(feature = "proto-2026-07-28-rc")))]
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
    #[cfg(feature = "tasks")]
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
    #[cfg(feature = "tasks")]
    pub(crate) fn get_task_status(&self, task_id: &str) -> Result<Task, Error> {
        self.tasks.get_status(task_id)
    }

    /// Awaits the task result
    #[cfg(feature = "tasks")]
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
        #[allow(unused_mut)]
        let mut cap = self.tools_capability.clone();
        // The stateless `proto-2026-07-28-rc` transport cannot push
        // `notifications/tools/list_changed`, so never advertise `listChanged`
        // under RC — clients refresh on cache-TTL / the next `tools/list`
        // instead of relying on a push that will never arrive.
        #[cfg(feature = "proto-2026-07-28-rc")]
        if let Some(c) = cap.as_mut() {
            c.list_changed = false;
        }
        cap
    }

    /// Returns [`ResourcesCapability`] if configured.
    /// If not configured but at least one [`Resource`] or [`ResourceTemplate`] exists, returns [`Default`].
    /// Otherwise, returns `None`.
    pub(crate) fn resources_capability(&self) -> Option<ResourcesCapability> {
        #[allow(unused_mut)]
        let mut cap = self.resources_capability.clone();
        // The stateless `proto-2026-07-28-rc` transport cannot push
        // `notifications/resources/updated` or `.../list_changed`, so mask both
        // `subscribe` and `listChanged` under RC. Subscribe handlers are also
        // not registered (see `App::new`), keeping the advertised surface and
        // the accepted methods in sync.
        #[cfg(feature = "proto-2026-07-28-rc")]
        if let Some(c) = cap.as_mut() {
            c.subscribe = false;
            c.list_changed = false;
        }
        cap
    }

    /// Returns [`PromptsCapability`] if configured.
    /// If not configured but at least one [`Prompt`] exists, returns [`Default`].
    /// Otherwise, returns `None`.
    pub(crate) fn prompts_capability(&self) -> Option<PromptsCapability> {
        #[allow(unused_mut)]
        let mut cap = self.prompts_capability.clone();
        // The stateless `proto-2026-07-28-rc` transport cannot push
        // `notifications/prompts/list_changed`, so never advertise `listChanged`
        // under RC — see `tools_capability` for the rationale.
        #[cfg(feature = "proto-2026-07-28-rc")]
        if let Some(c) = cap.as_mut() {
            c.list_changed = false;
        }
        cap
    }

    /// Returns [`ServerTasksCapability`] if configured.
    ///
    /// Otherwise, returns `None`.
    #[cfg(all(feature = "tasks", not(feature = "proto-2026-07-28-rc")))]
    pub(crate) fn tasks_capability(&self) -> Option<ServerTasksCapability> {
        self.tasks_capability.clone()
    }

    /// Returns the registered protocol extensions as a capability map
    /// (MCP 2026-07-28 RC), or `None` when no extension is registered so the
    /// `capabilities.extensions` field is omitted on the wire.
    #[cfg(feature = "proto-2026-07-28-rc")]
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
    #[cfg(feature = "tasks")]
    pub(crate) fn is_tasks_list_supported(&self) -> bool {
        self.tasks_capability
            .as_ref()
            .is_some_and(|tasks| tasks.list.is_some())
    }

    /// Returns whether the server is configured to handle the "tasks/cancel" requests.
    #[inline]
    #[cfg(feature = "tasks")]
    pub(crate) fn is_tasks_cancellation_supported(&self) -> bool {
        self.tasks_capability
            .as_ref()
            .is_some_and(|tasks| tasks.cancel.is_some())
    }

    /// Returns whether the server is configured to handle the task-augmented "tools/call" requests.
    #[inline]
    #[cfg(feature = "tasks")]
    pub(crate) fn is_task_augmented_tool_call_supported(&self) -> bool {
        self.tasks_capability
            .as_ref()
            .and_then(|tasks| tasks.requests.as_ref())
            .and_then(|req| req.tools.as_ref())
            .is_some_and(|tools| tools.call.is_some())
    }

    /// Sets the shared secret used to sign MRTR `requestState`.
    #[cfg(feature = "proto-2026-07-28-rc")]
    pub(crate) fn set_request_state_secret(&mut self, key: &[u8]) {
        self.request_state_secret = Arc::from(key);
        self.request_state_secret_explicit = true;
    }

    /// Returns the MRTR `requestState` signing key.
    #[cfg(feature = "proto-2026-07-28-rc")]
    pub(crate) fn request_state_secret(&self) -> &[u8] {
        &self.request_state_secret
    }

    /// Returns whether the MRTR `requestState` signing key was set explicitly
    /// (vs the ephemeral per-process default).
    ///
    /// Only compiled with `tracing`, where it backs the startup deployment
    /// warning in [`crate::App::run`]; without it the field has no reader.
    #[cfg(all(
        feature = "proto-2026-07-28-rc",
        feature = "http-server",
        feature = "tracing"
    ))]
    pub(crate) fn request_state_secret_is_explicit(&self) -> bool {
        self.request_state_secret_explicit
    }

    /// Returns whether the configured transport is the HTTP server transport.
    #[cfg(all(
        feature = "proto-2026-07-28-rc",
        feature = "http-server",
        feature = "tracing"
    ))]
    pub(crate) fn is_http_transport(&self) -> bool {
        matches!(self.proto, Some(TransportProto::HttpServer(_)))
    }

    /// Returns the MRTR `requestState` TTL in seconds.
    #[cfg(feature = "proto-2026-07-28-rc")]
    pub(crate) fn request_state_ttl_secs(&self) -> u64 {
        self.request_state_ttl_secs
    }

    /// Sets the max encoded `requestState` size in bytes.
    #[cfg(feature = "proto-2026-07-28-rc")]
    pub(crate) fn set_max_state_bytes(&mut self, bytes: usize) {
        self.max_state_bytes = bytes;
    }

    /// Returns the max encoded `requestState` size in bytes.
    #[cfg(feature = "proto-2026-07-28-rc")]
    pub(crate) fn max_state_bytes(&self) -> usize {
        self.max_state_bytes
    }

    /// Sets the MRTR final-round idempotency store.
    #[cfg(feature = "proto-2026-07-28-rc")]
    pub(crate) fn set_request_state_store(
        &mut self,
        store: Arc<dyn crate::app::mrtr_store::RequestStateStore>,
    ) {
        self.request_state_store = store;
    }

    /// Returns the MRTR final-round idempotency store.
    #[cfg(feature = "proto-2026-07-28-rc")]
    pub(crate) fn request_state_store(&self) -> &dyn crate::app::mrtr_store::RequestStateStore {
        self.request_state_store.as_ref()
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

        let result = prompt.call(req.into()).await.unwrap();

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

        let result = prompt.call(req.into()).await;

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

        // Under the stateless RC transport `listChanged` is masked off because
        // the server cannot push it; otherwise it round-trips the config.
        #[cfg(not(feature = "proto-2026-07-28-rc"))]
        assert!(tools_capability.list_changed);
        #[cfg(feature = "proto-2026-07-28-rc")]
        assert!(!tools_capability.list_changed);
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

        // Masked off under the stateless RC transport (see `tools` test above).
        #[cfg(not(feature = "proto-2026-07-28-rc"))]
        assert!(resources_capability.list_changed);
        #[cfg(feature = "proto-2026-07-28-rc")]
        assert!(!resources_capability.list_changed);
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
        feature = "proto-2026-07-28-rc",
        feature = "http-server",
        feature = "tracing"
    ))]
    #[test]
    fn request_state_secret_is_not_explicit_by_default() {
        let options = McpOptions::default();
        assert!(!options.request_state_secret_is_explicit());
    }

    #[cfg(all(
        feature = "proto-2026-07-28-rc",
        feature = "http-server",
        feature = "tracing"
    ))]
    #[test]
    fn request_state_secret_is_explicit_once_set() {
        let mut options = McpOptions::default();
        options.set_request_state_secret(b"shared-secret");
        assert!(options.request_state_secret_is_explicit());
    }

    #[test]
    fn it_returns_some_prompts_capability_if_configured() {
        let options = McpOptions::default().with_prompts(|prompts| prompts.with_list_changed());

        let prompts_capability = options.prompts_capability().unwrap();

        // Masked off under the stateless RC transport (see `tools` test above).
        #[cfg(not(feature = "proto-2026-07-28-rc"))]
        assert!(prompts_capability.list_changed);
        #[cfg(feature = "proto-2026-07-28-rc")]
        assert!(!prompts_capability.list_changed);
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

//! Represents an MCP application

use self::{
    context::{Context, ServerRuntime},
    options::{McpOptions, RuntimeMcpOptions},
};
use crate::app::handler::{
    CompletionHandler, FromHandlerParams, GenericHandler, HandlerParams, ListResourcesHandler,
    RequestFunc, RequestHandler,
};
use crate::error::{Error, ErrorCode};
use crate::middleware::{MwContext, Next, make_fn::make_mw};
use crate::shared;
use crate::transport::{Receiver, Sender, Transport};
use crate::types::{
    CallToolRequestParams, CallToolResponse, CompleteResult, FromHandlerArgs,
    GetPromptRequestParams, GetPromptResult, IntoResponse, ListPromptsRequestParams,
    ListPromptsResult, ListResourceTemplatesRequestParams, ListResourceTemplatesResult,
    ListResourcesRequestParams, ListResourcesResult, ListToolsRequestParams, ListToolsResult,
    Message, MessageBatch, MessageEnvelope, Prompt, PromptHandler, ReadResourceRequestParams,
    ReadResourceResult, Request, Resource, ResourceTemplate, Response, Tool, ToolHandler, Uri,
    notification::{CancelledNotificationParams, Notification},
    resource::template::ResourceFunc,
};
#[cfg(feature = "legacy-spec")]
use crate::types::{InitializeRequestParams, InitializeResult};
// The subscribe/unsubscribe RPCs exist only under the legacy transport; MCP
// 2026-07-28 folds them into the `subscriptions/listen` filter, so these params
// are unused there.
#[cfg(not(feature = "legacy-spec"))]
use crate::types::{RequestId, SubscriptionsListenRequestParams, SubscriptionsListenResult};
#[cfg(feature = "legacy-spec")]
use crate::types::{SubscribeRequestParams, UnsubscribeRequestParams};
use tokio_util::sync::CancellationToken;

#[cfg(all(feature = "tasks", feature = "legacy-spec"))]
use crate::types::Task;
#[cfg(feature = "tasks")]
use crate::types::{CancelTaskRequestParams, GetTaskRequestParams};
#[cfg(all(feature = "tasks", not(feature = "legacy-spec")))]
use crate::types::{DetailedTask, UpdateTaskRequestParams};
#[cfg(all(feature = "tasks", feature = "legacy-spec"))]
use crate::types::{
    GetTaskPayloadRequestParams, ListTasksRequestParams, ListTasksResult, TaskPayload,
    cursor::Pagination,
};
#[cfg(feature = "tasks")]
use context::ToolOrTaskResponse;

use std::{
    collections::HashMap,
    fmt::{Debug, Formatter},
    sync::Arc,
};

#[cfg(all(feature = "tracing", feature = "legacy-spec"))]
use crate::types::notification::SetLevelRequestParams;
#[cfg(feature = "tracing")]
use tracing::Instrument;
#[cfg(feature = "di")]
use volga_di::{Container, ContainerBuilder};

mod collection;
pub mod context;
#[cfg(not(feature = "legacy-spec"))]
pub mod extension;
mod greeter;
pub(crate) mod handler;
#[cfg(not(feature = "legacy-spec"))]
pub mod mrtr_store;
pub mod options;
#[cfg(not(feature = "legacy-spec"))]
pub(crate) mod subscriptions;

const DEFAULT_PAGE_SIZE: usize = 10;

type RequestHandlers = HashMap<String, RequestHandler<Response>>;

/// Represents an MCP server application
pub struct App {
    /// Whether to print the startup greeting banner
    greeting: bool,

    /// MCP server options
    pub(super) options: McpOptions,

    /// DI container
    #[cfg(feature = "di")]
    pub(super) container: ContainerBuilder,

    /// MCP server request handlers
    handlers: RequestHandlers,
}

impl Debug for App {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("App { ... }")
    }
}

impl Default for App {
    /// Creates a default [`App`] with all built-in handlers registered.
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// Initializes a new MCP app
    pub fn new() -> Self {
        let mut app = Self {
            greeting: cfg!(debug_assertions),
            options: McpOptions::default(),
            handlers: HashMap::new(),
            #[cfg(feature = "di")]
            container: ContainerBuilder::new(),
        };

        #[cfg(feature = "legacy-spec")]
        app.map_handler(crate::commands::INIT, Self::init);
        #[cfg(not(feature = "legacy-spec"))]
        app.map_handler(crate::commands::DISCOVER, Self::discover);
        app.map_handler(
            crate::types::completion::commands::COMPLETE,
            Self::completion,
        );

        app.map_handler(crate::types::tool::commands::LIST, Self::tools);
        app.map_handler(crate::types::tool::commands::CALL, Self::tool);

        app.map_handler(crate::types::resource::commands::LIST, Self::resources);
        app.map_handler(
            crate::types::resource::commands::TEMPLATES_LIST,
            Self::resource_templates,
        );
        app.map_handler(crate::types::resource::commands::READ, Self::resource);
        // MCP 2026-07-28 folds `resources/subscribe` into the
        // `subscriptions/listen` filter: a per-resource subscription is a URI
        // in `notifications.resourceSubscriptions`, scoped to that stream. The
        // two RPCs stay legacy-only.
        #[cfg(feature = "legacy-spec")]
        {
            app.map_handler(
                crate::types::resource::commands::SUBSCRIBE,
                Self::resource_subscribe,
            );
            app.map_handler(
                crate::types::resource::commands::UNSUBSCRIBE,
                Self::resource_unsubscribe,
            );
        }
        #[cfg(not(feature = "legacy-spec"))]
        app.map_handler(
            crate::types::subscription::commands::LISTEN,
            Self::subscriptions_listen,
        );

        app.map_handler(crate::types::prompt::commands::LIST, Self::prompts);
        app.map_handler(crate::types::prompt::commands::GET, Self::prompt);

        #[cfg(feature = "tasks")]
        {
            use crate::types::task::commands;
            app.map_handler(commands::GET, Self::task);
            app.map_handler(commands::CANCEL, Self::cancel_task);
            #[cfg(not(feature = "legacy-spec"))]
            app.map_handler(commands::UPDATE, Self::update_task);
            #[cfg(feature = "legacy-spec")]
            {
                app.map_handler(commands::LIST, Self::tasks);
                app.map_handler(commands::RESULT, Self::task_result);
            }
        }

        #[cfg(feature = "legacy-spec")]
        app.map_handler(crate::commands::PING, Self::ping);

        #[cfg(all(feature = "tracing", feature = "legacy-spec"))]
        app.map_handler(
            crate::types::notification::commands::SET_LOG_LEVEL,
            Self::set_log_level,
        );

        app
    }

    /// Starts the [`App`] with its own Tokio runtime.
    ///
    /// This method is intended for simple use cases where you don't already have a Tokio runtime setup.
    /// Internally, it creates and runs a multi-threaded Tokio runtime to execute the application.
    ///
    /// **Note:** This method **must not** be called from within an existing Tokio runtime
    /// (e.g., inside an `#[tokio::main]` async function), or it will panic.
    /// If you are already using Tokio in your application, use [`App::run`] instead.
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// # fn main() {
    /// let mut app = App::new();
    ///
    /// // configure tools, resources, prompts
    ///
    /// app.run_blocking()
    /// # }
    /// ```
    pub fn run_blocking(self) {
        if tokio::runtime::Handle::try_current().is_ok() {
            panic!(
                "`App::run_blocking()` cannot be called inside an existing Tokio runtime. Use `run().await` instead."
            );
        }

        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(err) => {
                #[cfg(feature = "tracing")]
                tracing::error!("failed to start the runtime: {err:#}");
                #[cfg(not(feature = "tracing"))]
                eprintln!("failed to start the runtime: {err:#}");
                return;
            }
        };

        runtime.block_on(async { self.run().await });
    }

    /// Run the MCP server
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut app = App::new();
    ///
    /// // configure tools, resources, prompts
    ///
    /// app.run().await;
    /// # }
    /// ```
    pub async fn run(mut self) {
        #[cfg(feature = "macros")]
        self.register_methods();

        // Must follow register_methods() for the same reason the greeting does:
        // macro-registered tools are not in the collection before it.
        self.validate_arg_names();

        // ORDERING CONSTRAINT: must execute after register_methods() so macro-registered
        // tools/prompts are present; must execute before self.options.transport() consumes
        // `proto` and before ServerRuntime::new() transitions collections to Runtime state
        // (Collection::as_ref() panics if called in Runtime state).
        if self.greeting {
            let transport_label = self.options.transport_label();
            let tools: Vec<String> = self.options.tools.as_ref().keys().cloned().collect();
            let prompts: Vec<String> = self.options.prompts.as_ref().keys().cloned().collect();
            let resource_templates: Vec<String> = self
                .options
                .resources_templates
                .as_ref()
                .keys()
                .cloned()
                .collect();

            greeter::Greeter {
                server_name: &self.options.implementation.name,
                server_version: &self.options.implementation.version,
                neva_version: env!("CARGO_PKG_VERSION"),
                transport_label: &transport_label,
                tools: &tools,
                prompts: &prompts,
                resource_templates: &resource_templates,
                use_color: std::env::var_os("NO_COLOR").is_none(),
            }
            .print();
        }

        // Multi-instance footgun guard: under the stateless 2026-07-28 HTTP transport an
        // MRTR retry can land on a different instance than the one that issued
        // the `requestState`. With the default ephemeral per-process secret that
        // retry fails `requestState` decryption/verification, which is a silent
        // prod failure. Warn at startup unless a shared secret was set explicitly.
        #[cfg(all(
            not(feature = "legacy-spec"),
            feature = "http-server",
            feature = "tracing"
        ))]
        if self.options.is_http_transport() && !self.options.request_state_secret_is_explicit() {
            tracing::warn!(
                "MRTR requestState is encrypted with an ephemeral per-process key. \
                 Multi-instance HTTP deployments MUST call \
                 App::with_request_state_secret(...) with a shared secret, or a \
                 retry routed to another instance will fail requestState \
                 verification."
            );
        }

        // The request tracing span must wrap the whole composed pipeline -- user
        // `wrap` middleware included -- so log events they emit around
        // `next(ctx)` stay inside the span and see the request-scoped level.
        // Prepending makes it the outermost layer; the terminal dispatcher
        // (`message_middleware`) stays innermost.
        #[cfg(feature = "tracing")]
        self.options
            .add_middleware_front(make_mw(Self::tracing_middleware));
        self.options
            .add_middleware(make_mw(Self::message_middleware));

        let mut transport = self.options.transport();
        let cancellation_token = transport.start();
        self.wait_for_shutdown_signal(cancellation_token.clone());
        // Long-lived requests (`subscriptions/listen`) watch the same signal
        // the dispatch loop breaks on, so they can close gracefully instead of
        // being dropped mid-stream.
        #[cfg(not(feature = "legacy-spec"))]
        self.options.set_shutdown_token(cancellation_token.clone());

        let (sender, mut receiver) = transport.split();
        let runtime = ServerRuntime::new(
            sender,
            self.options,
            self.handlers,
            #[cfg(feature = "di")]
            self.container.build(),
        );
        loop {
            tokio::select! {
                biased;
                _ = cancellation_token.cancelled() => break,
                msg = receiver.recv() => {
                    match msg {
                        Ok(msg) => match msg {
                            Message::Batch(batch) => {
                                tokio::spawn(Self::execute_batch(batch, runtime.clone()));
                            },
                            msg => {
                                tokio::spawn(Self::execute(msg, runtime.clone()));
                            }
                        },
                        Err(_err) => {
                            #[cfg(feature = "tracing")]
                            tracing::error!("Error handling message: {:?}", _err);
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Sets the shared secret used to encrypt and authenticate MRTR
    /// `requestState` (MCP 2026-07-28).
    ///
    /// The blob is sealed with ChaCha20-Poly1305 (AEAD) using a key derived from
    /// this secret, so the payload -- including any values a handler caches via
    /// [`Context::memo`](crate::Context::memo) -- is confidential as well as
    /// tamper-evident.
    ///
    /// **Multi-instance stateless deployments MUST set this to a shared
    /// secret** -- otherwise a retry that lands on a different instance fails to
    /// decrypt the `requestState`. If unset, an ephemeral per-process key is
    /// used (fine for single-instance / development).
    ///
    /// This is the single-key shorthand (kid `"0"`); for key rotation use
    /// [`with_request_state_keys`](Self::with_request_state_keys).
    ///
    /// Because the state is sealed rather than signed, this value protects the
    /// *confidentiality* of memoized values, not just their integrity -- treat
    /// it as a secret. See the [state codec docs](crate::types::mrtr) for why
    /// MRTR encrypts instead of signing.
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(not(feature = "legacy-spec"))] {
    /// use neva::App;
    ///
    /// let app = App::new()
    ///     .with_request_state_secret(b"shared-secret");
    /// # }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub fn with_request_state_secret(mut self, secret: impl AsRef<[u8]>) -> Self {
        self.options.set_request_state_secret(secret.as_ref());
        self
    }

    /// Sets the keyring used to encrypt and authenticate MRTR `requestState`,
    /// enabling zero-downtime key rotation (MCP 2026-07-28).
    ///
    /// New blobs are sealed with the key `active_kid` names and carry the kid
    /// on the wire (`v1.{kid}....`); an inbound blob is decrypted with whichever
    /// accepted key its kid segment names. To rotate: ship the new key as
    /// *accepted* on every instance first, then flip `active_kid` to it --
    /// states minted under the old kid keep verifying until their TTL lapses,
    /// after which the old key can be dropped.
    ///
    /// `active_kid` must name one of `keys` (encoding fails otherwise), be
    /// non-empty and contain no `'.'` (the wire segment separator).
    /// [`with_request_state_secret`](Self::with_request_state_secret) is the
    /// single-key shorthand (kid `"0"`).
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(not(feature = "legacy-spec"))] {
    /// use neva::App;
    ///
    /// let app = App::new()
    ///     .with_request_state_keys("2", [
    ///         ("2", b"new-shared-secret".as_slice()),
    ///         ("1", b"old-shared-secret".as_slice()),
    ///     ]);
    /// # }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub fn with_request_state_keys<K, S>(
        mut self,
        active_kid: impl AsRef<str>,
        keys: impl IntoIterator<Item = (K, S)>,
    ) -> Self
    where
        K: AsRef<str>,
        S: AsRef<[u8]>,
    {
        self.options
            .set_request_state_keys(active_kid.as_ref(), keys);
        self
    }

    /// Sets the maximum encoded `requestState` size (bytes). When a round-trip
    /// would emit a larger blob, the server returns an error result instead
    /// (MCP 2026-07-28).
    ///
    /// Defaults to 8 KiB. Lower it to push handlers toward [`crate::Context::once`]
    /// (key-only) over [`crate::Context::memo`] (serialized value); raise it for
    /// memo-heavy flows.
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(not(feature = "legacy-spec"))] {
    /// use neva::App;
    ///
    /// let app = App::new()
    ///     .with_max_state_bytes(16 * 1024);
    /// # }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub fn with_max_state_bytes(mut self, bytes: usize) -> Self {
        self.options.set_max_state_bytes(bytes);
        self
    }

    /// Sets the store backing MRTR final-round idempotency
    /// (MCP 2026-07-28).
    ///
    /// When the final round of an MRTR flow commits but its HTTP response is
    /// lost, the client retries the same `requestState`; the store lets the
    /// server return the already-computed response instead of re-running the
    /// handler (and its [`Context::on_commit`](crate::Context::on_commit) /
    /// [`Context::once`](crate::Context::once) side effects) a second time.
    ///
    /// Defaults to a per-process
    /// [`InMemoryStateStore`](crate::app::mrtr_store::InMemoryStateStore).
    /// **Multi-instance stateless deployments should set a shared store** (e.g.
    /// Redis) so a retry routed to another instance still sees the committed
    /// result -- the same constraint as
    /// [`with_request_state_secret`](Self::with_request_state_secret).
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(not(feature = "legacy-spec"))] {
    /// use neva::App;
    /// use neva::app::mrtr_store::InMemoryStateStore;
    ///
    /// let app = App::new()
    ///     .with_request_state_store(InMemoryStateStore::new());
    /// # }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub fn with_request_state_store(
        mut self,
        store: impl crate::app::mrtr_store::RequestStateStore + 'static,
    ) -> Self {
        self.options
            .set_request_state_store(std::sync::Arc::new(store));
        self
    }

    /// Registers a protocol [`Extension`](crate::app::extension::Extension)
    /// (MCP 2026-07-28).
    ///
    /// Records the extension's capability under its reverse-DNS id (surfaced by
    /// `server/discover` under `capabilities.extensions`) and lets it register
    /// its request handlers. This is the generic entry point for extensions;
    /// the built-in Tasks extension is also reachable through `with_tasks`.
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(all(not(feature = "legacy-spec"), feature = "tasks"))] {
    /// use neva::App;
    /// use neva::app::extension::TasksExtension;
    /// use neva::types::ServerTasksCapability;
    /// let app = App::new()
    ///     .with_extension(TasksExtension::new(ServerTasksCapability::default()));
    /// # }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub fn with_extension<E: crate::app::extension::Extension>(mut self, ext: E) -> Self {
        self.options.register_extension(ext.id(), ext.capability());
        ext.register(&mut self);
        self
    }

    /// Enable the greeting banner on startup (forced on, even in release builds).
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// # fn main() {
    /// let app = App::new().with_greeting();
    /// # }
    /// ```
    pub fn with_greeting(mut self) -> Self {
        self.greeting = true;
        self
    }

    /// Suppress the greeting banner on startup.
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// # fn main() {
    /// let app = App::new().without_greeting();
    /// # }
    /// ```
    pub fn without_greeting(mut self) -> Self {
        self.greeting = false;
        self
    }

    /// Configure MCP server options
    pub fn with_options<F>(mut self, config: F) -> Self
    where
        F: FnOnce(McpOptions) -> McpOptions,
    {
        self.options = config(self.options);
        self
    }

    /// Maps an MCP client request to a specific function
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut app = App::new();
    ///
    /// app.map_handler("ping", || async {
    ///     "pong"
    /// });
    ///
    /// # app.run().await;
    /// # }
    /// ```
    pub fn map_handler<F, R, Args>(&mut self, name: impl Into<String>, handler: F) -> &mut Self
    where
        F: GenericHandler<Args, Output = R>,
        R: IntoResponse + Send + 'static,
        Args: FromHandlerParams + Send + Sync + 'static,
    {
        let handler = RequestFunc::new(handler);
        self.handlers.insert(name.into(), handler);
        self
    }

    /// Fails startup when a registered tool or prompt and its handler disagree
    /// about the arguments.
    ///
    /// Such a primitive cannot be called successfully by anyone, so it is a
    /// registration mistake worth reporting before the server starts serving
    /// rather than on a peer's first call. See [`Tool::arg_name_conflict`] and
    /// [`Prompt::arg_name_conflict`].
    fn validate_arg_names(&self) {
        let conflict = self
            .options
            .tools
            .as_ref()
            .values()
            .find_map(Tool::arg_name_conflict)
            .or_else(|| {
                self.options
                    .prompts
                    .as_ref()
                    .values()
                    .find_map(Prompt::arg_name_conflict)
            });

        if let Some(conflict) = conflict {
            panic!("{conflict}");
        }
    }

    /// Maps an MCP tool call request to a specific function and returns a mutable reference to the
    /// [`Tool`] for further configuration
    ///
    /// # Argument names
    ///
    /// A call's `arguments` are read **by name**. Rust does not keep a
    /// closure's parameter names, so a tool registered this way publishes and
    /// reads the positional `arg0`, `arg1`, ... names. To give them the names
    /// you wrote, use [`Tool::with_arg_names`], the [`crate::map_tool`] macro,
    /// or the `#[tool]` attribute -- each declares the names and renames the
    /// published schema together, so the two cannot disagree.
    ///
    /// Overriding the schema with [`Tool::with_input_schema`] does *not* by
    /// itself change what the handler reads; a tool left in that state fails
    /// [`App::run`] at startup rather than on a peer's first call.
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut app = App::new();
    ///
    /// app.map_tool("hello", |name: String| async move {
    ///     format!("Hello, {name}")
    /// })
    /// .with_arg_names(["name"]);
    ///
    /// # app.run().await;
    /// # }
    /// ```
    pub fn map_tool<F, R, Args>(&mut self, name: impl Into<String>, handler: F) -> &mut Tool
    where
        F: ToolHandler<Args, Output = R>,
        R: Into<CallToolResponse> + Send + 'static,
        Args: FromHandlerArgs<CallToolRequestParams> + Send + Sync + 'static,
    {
        self.options.add_tool(Tool::new(name, handler))
    }

    /// Adds a known resource
    pub fn add_resource<U: Into<Uri>, S: Into<String>>(
        &mut self,
        uri: U,
        name: S,
    ) -> &mut Resource {
        let resource = Resource::new(uri, name);
        self.options.add_resource(resource)
    }

    /// Maps an MCP resource read request to a specific function
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut app = App::new();
    ///
    /// app.map_resource("res://{name}", "read_resource", |name: String| async move {
    ///     (format!("res://{name}"), format!("Resource: {name} content"))
    /// });
    ///
    /// # app.run().await;
    /// # }
    /// ```
    pub fn map_resource<F, R, Args>(
        &mut self,
        uri: impl Into<Uri>,
        name: impl Into<String>,
        handler: F,
    ) -> &mut ResourceTemplate
    where
        F: GenericHandler<Args, Output = R>,
        R: TryInto<ReadResourceResult> + Send + 'static,
        R::Error: Into<Error>,
        Args: TryFrom<ReadResourceRequestParams, Error = Error> + Send + Sync + 'static,
    {
        let handler = ResourceFunc::new(handler);
        let template = ResourceTemplate::new(uri, name);

        self.options.add_resource_template(template, handler)
    }

    /// Maps an MCP get a prompt request to a specific function
    ///
    /// # Argument names
    ///
    /// A request's `arguments` are read **by name**, and Rust does not keep a
    /// closure's parameter names -- a prompt registered this way publishes and
    /// reads the positional `arg0`, `arg1`, ... names until
    /// [`Prompt::with_args`] gives them yours. [`crate::map_prompt`] and the
    /// `#[prompt]` attribute do that for you.
    ///
    /// # Example
    /// ```no_run
    /// use neva::{App, types::Role};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut app = App::new();
    ///
    /// app.map_prompt("analyze-code", |lang: String| async move {
    ///     (format!("Language: {lang}"), Role::User)
    /// })
    /// .with_args(["lang"]);
    ///
    /// # app.run().await;
    /// # }
    /// ```
    pub fn map_prompt<F, R, Args>(&mut self, name: impl Into<String>, handler: F) -> &mut Prompt
    where
        F: PromptHandler<Args, Output = R>,
        R: TryInto<GetPromptResult> + Send + 'static,
        R::Error: Into<Error>,
        Args: FromHandlerArgs<GetPromptRequestParams> + Send + Sync + 'static,
    {
        self.options.add_prompt(Prompt::new(name, handler))
    }

    /// Maps an MCP resource read request to a specific function
    ///
    /// # Example
    /// ```no_run
    /// use neva::{App, types::{Resource, ListResourcesRequestParams}};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut app = App::new();
    ///
    /// app.map_resources(|_params: ListResourcesRequestParams| async move {
    ///     [
    ///         Resource::new("res://res1", "res1"),
    ///         Resource::new("res://res2", "res2")
    ///     ]
    /// });
    ///
    /// # app.run().await;
    /// # }
    /// ```
    pub fn map_resources<F, Args, R>(&mut self, handler: F) -> &mut Self
    where
        F: ListResourcesHandler<Args, Output = R> + Clone + Send + Sync + 'static,
        Args: FromHandlerParams + Send + Sync + 'static,
        R: Into<ListResourcesResult>,
    {
        let handler = move |params, args| {
            let handler = handler.clone();
            async move { handler.call(params, args).await.into() }
        };
        self.map_handler(crate::types::resource::commands::LIST, handler);
        self
    }

    /// Maps a completion request
    ///
    /// # Example
    /// ```no_run
    /// use neva::{App, types::{CompleteRequestParams, CompleteResult}};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let mut app = App::new();
    ///
    /// app.map_completion(|_params: CompleteRequestParams| async move {
    ///     ["Item 1", "Item 2", "Item 3"]
    /// });
    ///
    /// # app.run().await;
    /// # }
    /// ```
    pub fn map_completion<F, Args, R>(&mut self, handler: F) -> &mut Self
    where
        F: CompletionHandler<Args, Output = R> + Clone + Send + Sync + 'static,
        Args: FromHandlerParams + Send + Sync + 'static,
        R: Into<CompleteResult>,
    {
        let handler = move |params, args| {
            let handler = handler.clone();
            async move { handler.call(params, args).await.into() }
        };
        self.map_handler(crate::types::completion::commands::COMPLETE, handler);
        self
    }

    /// Connection initialization handler (legacy handshake).
    #[cfg(feature = "legacy-spec")]
    async fn init(
        options: RuntimeMcpOptions,
        _params: InitializeRequestParams,
    ) -> Result<InitializeResult, Error> {
        Ok(InitializeResult::new(&options))
    }

    /// Stateless capability discovery handler (MCP 2026-07-28).
    #[cfg(not(feature = "legacy-spec"))]
    async fn discover(
        options: RuntimeMcpOptions,
        _params: crate::types::DiscoverRequestParams,
    ) -> Result<crate::types::DiscoverResult, Error> {
        Ok(crate::types::DiscoverResult::new(&options))
    }

    /// Completion request handler
    async fn completion() -> CompleteResult {
        // return default as its non-optional capability so far
        CompleteResult::default()
    }

    /// Tools request handler
    #[cfg_attr(feature = "legacy-spec", allow(clippy::needless_update))]
    async fn tools(options: RuntimeMcpOptions, params: ListToolsRequestParams) -> ListToolsResult {
        let (tools, next_cursor) = options
            .list_tools_page(params.cursor, DEFAULT_PAGE_SIZE)
            .await;
        ListToolsResult {
            tools,
            next_cursor,
            ..Default::default()
        }
    }

    /// Resources request handler
    #[cfg_attr(feature = "legacy-spec", allow(clippy::needless_update))]
    async fn resources(
        options: RuntimeMcpOptions,
        params: ListResourcesRequestParams,
    ) -> ListResourcesResult {
        let (resources, next_cursor) = options
            .list_resources_page(params.cursor, DEFAULT_PAGE_SIZE)
            .await;
        ListResourcesResult {
            resources,
            next_cursor,
            ..Default::default()
        }
    }

    /// Resource templates request handler
    #[cfg_attr(feature = "legacy-spec", allow(clippy::needless_update))]
    async fn resource_templates(
        options: RuntimeMcpOptions,
        params: ListResourceTemplatesRequestParams,
    ) -> ListResourceTemplatesResult {
        let (resource_templates, next_cursor) = options
            .list_resource_templates_page(params.cursor, DEFAULT_PAGE_SIZE)
            .await;
        ListResourceTemplatesResult {
            templates: resource_templates,
            next_cursor,
            ..Default::default()
        }
    }

    /// Prompts request handler
    #[cfg_attr(feature = "legacy-spec", allow(clippy::needless_update))]
    async fn prompts(
        options: RuntimeMcpOptions,
        params: ListPromptsRequestParams,
    ) -> ListPromptsResult {
        let (prompts, next_cursor) = options
            .list_prompts_page(params.cursor, DEFAULT_PAGE_SIZE)
            .await;
        ListPromptsResult {
            prompts,
            next_cursor,
            ..Default::default()
        }
    }

    /// A tool call request handler
    #[cfg(not(feature = "tasks"))]
    async fn tool(ctx: Context, params: CallToolRequestParams) -> Result<CallToolResponse, Error> {
        ctx.call_tool(params).await
    }

    /// A tool call request handler
    #[cfg(feature = "tasks")]
    async fn tool(
        ctx: Context,
        params: CallToolRequestParams,
    ) -> Result<ToolOrTaskResponse, Error> {
        ctx.call_tool_with_task(params).await
    }

    /// A read resource request handler
    async fn resource(
        ctx: Context,
        params: ReadResourceRequestParams,
    ) -> Result<ReadResourceResult, Error> {
        ctx.read_resource(params).await
    }

    /// A get prompt request handler
    async fn prompt(
        ctx: Context,
        params: GetPromptRequestParams,
    ) -> Result<GetPromptResult, Error> {
        ctx.get_prompt(params).await
    }

    /// Ping request handler
    #[cfg(feature = "legacy-spec")]
    async fn ping() {}

    /// A subscription to a resource change request handler
    ///
    /// Not registered under MCP 2026-07-28, where the method is folded into the
    /// `subscriptions/listen` filter; see [`Self::subscriptions_listen`].
    #[cfg(feature = "legacy-spec")]
    async fn resource_subscribe(mut ctx: Context, params: SubscribeRequestParams) {
        ctx.subscribe_to_resource(params.uri);
    }

    /// An unsubscription to from resource change request handler
    ///
    /// Not registered under MCP 2026-07-28; see [`Self::resource_subscribe`].
    #[cfg(feature = "legacy-spec")]
    async fn resource_unsubscribe(mut ctx: Context, params: UnsubscribeRequestParams) {
        ctx.unsubscribe_from_resource(&params.uri);
    }

    /// A `subscriptions/listen` request handler (MCP 2026-07-28).
    ///
    /// The request stays open for the life of the subscription: the accepted
    /// filter is acknowledged first, notifications matching it flow on the same
    /// stream, and the reply -- an empty result carrying the subscription id --
    /// is what marks a graceful close.
    #[cfg(not(feature = "legacy-spec"))]
    async fn subscriptions_listen(
        ctx: Context,
        id: RequestId,
        params: SubscriptionsListenRequestParams,
    ) -> Result<SubscriptionsListenResult, Error> {
        ctx.listen(id, params.notifications).await
    }

    /// Tasks request handler
    ///
    /// Not registered under MCP 2026-07-28: the final Tasks extension has no
    /// `tasks/list`. A task id is a durable handle the requestor already holds.
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    async fn tasks(
        options: RuntimeMcpOptions,
        params: ListTasksRequestParams,
    ) -> Result<ListTasksResult, Error> {
        if !options.is_tasks_list_supported() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "Server does not support support tasks/list requests.",
            ));
        }
        Ok(options
            .list_tasks()
            .paginate(params.cursor, DEFAULT_PAGE_SIZE)
            .into())
    }

    /// A cancel task request handler
    ///
    /// Under MCP 2026-07-28 cancellation is cooperative and the reply is an
    /// empty acknowledgement -- the requestor learns the outcome by polling
    /// `tasks/get`, since the task may still reach a non-`cancelled` terminal
    /// status. `legacy-spec` returns the cancelled [`Task`] instead.
    #[cfg(all(feature = "tasks", not(feature = "legacy-spec")))]
    async fn cancel_task(
        options: RuntimeMcpOptions,
        params: CancelTaskRequestParams,
    ) -> Result<(), Error> {
        options.cancel_task(&params.id).map(|_| ())
    }

    /// A cancel task request handler
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    async fn cancel_task(
        options: RuntimeMcpOptions,
        params: CancelTaskRequestParams,
    ) -> Result<Task, Error> {
        if options.is_tasks_cancellation_supported() {
            options.cancel_task(&params.id)
        } else {
            Err(Error::new(
                ErrorCode::InvalidRequest,
                "Server does not support support tasks/cancel requests.",
            ))
        }
    }

    /// A task state retrieval request handler
    ///
    /// Under MCP 2026-07-28 this is the single polling method: the reply
    /// carries the status plus, depending on it, the outstanding
    /// `inputRequests`, the terminal `result`, or the `error`.
    #[cfg(all(feature = "tasks", not(feature = "legacy-spec")))]
    async fn task(
        options: RuntimeMcpOptions,
        params: GetTaskRequestParams,
    ) -> Result<DetailedTask, Error> {
        options.get_task_state(&params.id)
    }

    /// A task status retrieval request handler
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    async fn task(options: RuntimeMcpOptions, params: GetTaskRequestParams) -> Result<Task, Error> {
        options.get_task_status(&params.id)
    }

    /// A task input submission request handler (MCP 2026-07-28)
    ///
    /// Answers the task's outstanding input requests and acknowledges with an
    /// empty result. Responses for unknown or already-satisfied keys are
    /// ignored, per the spec.
    #[cfg(all(feature = "tasks", not(feature = "legacy-spec")))]
    async fn update_task(
        options: RuntimeMcpOptions,
        params: UpdateTaskRequestParams,
    ) -> Result<(), Error> {
        options.update_task(&params.id, params.input_responses)
    }

    /// A task result retrieval request handler
    ///
    /// Not registered under MCP 2026-07-28: result retrieval folded into
    /// `tasks/get`.
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    async fn task_result(
        options: RuntimeMcpOptions,
        params: GetTaskPayloadRequestParams,
    ) -> Result<TaskPayload, Error> {
        options.get_task_result(&params.id).await
    }

    /// Sets the logging level
    #[allow(deprecated)]
    #[cfg(all(feature = "tracing", feature = "legacy-spec"))]
    async fn set_log_level(
        options: RuntimeMcpOptions,
        params: SetLevelRequestParams,
    ) -> Result<(), Error> {
        let current_level = options.log_level();
        tracing::debug!(
            logger = "neva",
            "Logging level has been changed from {:?} to {:?}",
            current_level,
            params.level
        );

        options.set_log_level(params.level)
    }

    #[cfg(feature = "tracing")]
    async fn tracing_middleware(ctx: MwContext, next: Next) -> Response {
        #[cfg(feature = "legacy-spec")]
        let span = create_tracing_span(ctx.session_id().cloned());
        // 2026-07-28: the request-scoped logging level rides on the originating
        // request's `_meta`. Stamp it onto the span so the notification layer
        // can decide which `notifications/message` to deliver for this request.
        #[cfg(not(feature = "legacy-spec"))]
        let span = {
            let log_level = ctx
                .request()
                .and_then(|req| req.meta())
                .and_then(|meta| meta.log_level);
            create_tracing_span(ctx.session_id().cloned(), log_level)
        };
        next(ctx).instrument(span).await
    }

    #[inline]
    async fn execute(msg: Message, runtime: ServerRuntime) {
        // Closing the request notification sink here -- once the *whole*
        // middleware pipeline has run -- is what lets a request-scoped SSE POST
        // response know no more notifications are coming, so logs emitted by
        // user middleware after `next(ctx)` still make it onto the stream.
        #[cfg(all(not(feature = "legacy-spec"), feature = "http-server"))]
        let _sink_guard = RequestSinkGuard(msg.session_id().copied());
        runtime.execute(msg).await;
    }

    async fn execute_batch(batch: MessageBatch, runtime: ServerRuntime) {
        use crate::transport::TransportProtoSender;
        use futures_util::future::join_all;

        // Capture the incoming batch's correlation and HTTP-context fields.
        // `id` + `session_id` are needed so the response batch can be routed
        // back to the correct waiting HTTP handler.  `headers` and `claims`
        // are copied onto every inner Request so that middleware (auth checks,
        // role/permission guards, SSE routing) sees the original HTTP context.
        let batch_id = batch.id.clone();
        let batch_session_id = batch.session_id;
        // One guard for the whole batch (inner requests run through
        // `ServerRuntime::execute`, so they never close the shared sink early):
        // the request-scoped SSE response stays open until every inner request
        // and its middleware have finished. See `App::execute`.
        #[cfg(all(not(feature = "legacy-spec"), feature = "http-server"))]
        let _sink_guard = RequestSinkGuard(batch_session_id);
        #[cfg(feature = "http-server")]
        let batch_headers = batch.headers.clone();
        #[cfg(feature = "http-server")]
        let batch_claims = batch.claims.clone();

        let real_sender = runtime.sender();

        // Collect responses produced by batch request handlers in-memory.
        // Server-initiated messages (sampling, elicitation, notifications) go
        // straight to the real transport inside BatchCollect::send, so handlers
        // that call ctx.elicit()/ctx.sample() never deadlock.
        //
        // Crucially, background tasks that capture a BatchCollect sender clone
        // do NOT block the batch response: we only wait for the join_all futures,
        // then snapshot whatever responses have been collected so far.
        let responses: Arc<std::sync::Mutex<Vec<MessageEnvelope>>> = Arc::default();
        let batch_sender = TransportProtoSender::BatchCollect {
            real_sender: Arc::new(tokio::sync::Mutex::new(real_sender.clone())),
            responses: Arc::clone(&responses),
        };

        // Capture before consuming the batch so we know whether to send an ack
        // when all Response envelopes were consumed by pending.complete (section below).
        let has_error_responses = batch
            .iter()
            .any(|e| matches!(e, MessageEnvelope::Response(Response::Err(_))));

        let futures = batch.into_iter().map(|envelope| {
            let runtime = runtime.clone();
            let mut sender = batch_sender.clone();
            // Clone per-iteration so each async move block owns its own copy.
            #[cfg(feature = "http-server")]
            let batch_headers = batch_headers.clone();
            #[cfg(feature = "http-server")]
            let batch_claims = batch_claims.clone();
            async move {
                match envelope {
                    MessageEnvelope::Request(mut req) => {
                        // Copy the batch's HTTP metadata onto the inner request
                        // so that session/auth context is preserved: without
                        // this, role/permission checks can fail with a valid
                        // token and server-initiated follow-up calls (sampling,
                        // elicitation) cannot be routed back over SSE.
                        req.session_id = batch_session_id;
                        #[cfg(feature = "http-server")]
                        {
                            req.headers = batch_headers;
                            req.claims = batch_claims;
                        }
                        // Route through the full middleware chain with the
                        // batch-collect sender so registered middlewares apply.
                        runtime
                            .with_sender(sender)
                            .execute(Message::Request(req))
                            .await;
                    }
                    MessageEnvelope::Notification(notification) => {
                        Self::handle_notification(notification, runtime.clone()).await;
                    }
                    MessageEnvelope::Response(mut resp) => {
                        // Apply the batch's session context so that
                        // `resp.full_id()` (= session_id + resp_id) matches
                        // the key used when the server registered the pending
                        // request via `send_request`. Without this the lookup
                        // in the pending queue misses and the pending handler leaks.
                        if let Some(session_id) = batch_session_id {
                            resp = resp.set_session_id(session_id);
                        }
                        #[cfg(feature = "http-server")]
                        {
                            resp = resp.set_headers(batch_headers);
                        }
                        // If a pending server-initiated request matches this id,
                        // complete it (the client is responding to a server request
                        // inside the batch). Otherwise, if the response carries an
                        // error, it is a synthetic InvalidRequest injected by the
                        // deserializer for a malformed batch item -- route it through
                        // the collector so it appears in the batch reply.
                        // Unmatched Ok responses are unsolicited or stale and are
                        // dropped silently, consistent with the single-message
                        // handle_response path.
                        if let Some(handle) = runtime.pending_requests().pop(&resp.full_id()) {
                            handle.send(resp);
                        } else if matches!(resp, Response::Err(_)) {
                            let _ = sender.send(Message::Response(resp)).await;
                        }
                    }
                }
            }
        });

        join_all(futures).await;

        // Snapshot collected responses. Any response that a background task
        // produces after this point is silently discarded -- it arrived too
        // late to be included in the batch reply.
        let envelopes = responses
            .lock()
            .map(|mut guard| std::mem::take(&mut *guard))
            .unwrap_or_default();

        if envelopes.is_empty() {
            if has_error_responses {
                // All Response::Err items were legitimate peer error responses
                // consumed by pending.complete above. If the HTTP transport
                // created a pending slot for this batch (because
                // `has_error_responses()` was true), we must close it;
                // otherwise the HTTP handler will block forever waiting for a
                // reply that never comes.
                let mut ack = Response::empty(batch_id);
                if let Some(session_id) = batch_session_id {
                    ack = ack.set_session_id(session_id);
                }
                let mut sender = real_sender;
                let _ = sender.send(Message::Response(ack)).await;
            }
            return;
        }

        let mut resp_batch = match MessageBatch::new(envelopes) {
            Ok(b) => b,
            Err(_err) => {
                // Unreachable in practice: envelopes are non-empty above.
                #[cfg(feature = "tracing")]
                tracing::error!(
                    logger = "neva",
                    "Failed to construct batch response: {:?}",
                    _err
                );
                return;
            }
        };
        // Restore the correlation id+session so the HTTP transport can match
        // this response batch to the waiting HTTP handler.
        resp_batch.id = batch_id;
        resp_batch.session_id = batch_session_id;

        let mut sender = real_sender;
        if let Err(_err) = sender.send(Message::Batch(resp_batch)).await {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "Error sending batch response: {:?}", _err);
        }
    }

    async fn message_middleware(ctx: MwContext, _: Next) -> Response {
        let MwContext {
            msg,
            runtime,
            #[cfg(feature = "di")]
            scope,
        } = ctx;
        let id = msg.id();
        let mut sender = runtime.sender();

        if let Some(resp) = Self::handle_message(
            msg,
            runtime,
            #[cfg(feature = "di")]
            scope,
        )
        .await
            && let Err(_err) = sender.send(resp.into()).await
        {
            #[cfg(feature = "tracing")]
            tracing::error!(
                logger = "neva",
                error = format!("Error sending response: {:?}", _err)
            );
        }

        Response::empty(id)
    }

    #[inline]
    async fn handle_message(
        msg: Message,
        runtime: ServerRuntime,
        #[cfg(feature = "di")] scope: Container,
    ) -> Option<Response> {
        match msg {
            Message::Request(req) => Some(
                Self::handle_request(
                    req,
                    runtime,
                    #[cfg(feature = "di")]
                    scope,
                )
                .await,
            ),
            Message::Response(resp) => Some(Self::handle_response(resp, runtime).await),
            Message::Notification(notification) => {
                // JSON-RPC 2.0 section 4: notifications must never receive a response.
                Self::handle_notification(notification, runtime).await;
                None
            }
            Message::Batch(_) => {
                // Batches are dispatched via execute_batch before reaching handle_message
                unreachable!(
                    "Message::Batch should be intercepted in App::run before handle_message"
                )
            }
        }
    }

    async fn handle_request(
        req: Request,
        runtime: ServerRuntime,
        #[cfg(feature = "di")] scope: Container,
    ) -> Response {
        #[cfg(feature = "http-server")]
        let mut req = req;
        let req_id = req.id();
        let session_id = req.session_id;
        let full_id = req.full_id();

        // MRTR pre-capture: method + salient params (params minus `_meta`),
        // needed after `req`/`context` are moved into `handler.call`.
        #[cfg(not(feature = "legacy-spec"))]
        let mrtr_method = shared::is_mrtr_method(&req.method);
        #[cfg(not(feature = "legacy-spec"))]
        let req_method = req.method.clone();
        #[cfg(not(feature = "legacy-spec"))]
        let salient_params = req
            .params
            .as_ref()
            .map(strip_meta)
            .unwrap_or(serde_json::Value::Null);

        // What MCP 2026-07-28 requires of every request's `_meta`: the
        // mandatory fields, and a protocol version this build actually speaks.
        // The HTTP preamble rejects both earlier so it can attach the `400` the
        // spec asks for; this seam is what every other transport gets, since
        // the requirements are on the message and not on how it travelled.
        #[cfg(not(feature = "legacy-spec"))]
        if let Some(err) = req
            .required_meta_error()
            .or_else(|| req.unsupported_version_error())
        {
            let mut resp = Response::error(req_id, err);
            if let Some(session_id) = session_id {
                resp = resp.set_session_id(session_id);
            }
            return resp;
        }

        // The `Mcp-Param-*` half of header validation. The transport preamble
        // checks the standard routing headers, but these are defined by the
        // called tool's own `x-mcp-header` annotations -- which only this side
        // of the channel knows -- so the check lands where the tool registry
        // does. The resulting `-32020` picks up its mandated `400` from the
        // transport's status mapping on the way out.
        #[cfg(all(feature = "http-server", not(feature = "legacy-spec")))]
        if let Some(err) = param_header_error(&req, &runtime.options()).await {
            let mut resp = Response::error(req_id, err);
            // The transport correlates a reply by `session_id`+`id`; a reply
            // that leaves without one is never matched to the POST waiting for
            // it. Every other return path down this function does the same.
            if let Some(session_id) = session_id {
                resp = resp.set_session_id(session_id);
            }
            return resp;
        }

        #[cfg(not(feature = "http-server"))]
        let context = runtime.context(session_id);

        #[cfg(feature = "http-server")]
        let context = {
            let headers = std::mem::take(&mut req.headers);
            let claims = req.claims.take();
            runtime.context(session_id, headers, claims)
        };

        #[cfg(feature = "di")]
        let context = context.with_scope(scope);

        let options = runtime.options();
        let handlers = runtime.request_handlers();
        let token = options.track_request(&full_id);

        // MRTR seed: decode/verify any incoming `requestState`, merge this
        // round's `inputResponses`, and attach the replay state to the context.
        #[cfg(not(feature = "legacy-spec"))]
        let mut context = context;
        #[cfg(not(feature = "legacy-spec"))]
        let (mrtr_arc, mrtr_principal) = if mrtr_method {
            #[cfg(feature = "http-server")]
            let principal = context
                .claims
                .as_ref()
                .and_then(|c| c.subject().map(|s| s.to_owned()));
            #[cfg(not(feature = "http-server"))]
            let principal: Option<String> = None;

            match seed_mrtr_ctx(
                &req,
                &req_method,
                &salient_params,
                &options,
                principal.as_deref(),
            ) {
                Ok(arc) => {
                    context.exec = crate::app::context::ExecMode::Mrtr(arc.clone());
                    (Some(arc), principal)
                }
                Err(e) => {
                    options.complete_request(&full_id);
                    let mut resp = Err::<Response, _>(e).into_response(req_id);
                    if let Some(session_id) = session_id {
                        resp = resp.set_session_id(session_id);
                    }
                    return resp;
                }
            }
        } else {
            (None, None)
        };

        // MRTR idempotency: the final-round response cache is keyed by the
        // incoming state's sealed segment (the ciphertext+tag after the last
        // `.` -- `rsplit_once` keeps grabbing it regardless of the leading
        // `v1.kid.` header segments -- unique per minted state thanks to the
        // random AEAD nonce) *plus* a
        // digest of this round's `inputResponses`. The answers digest matters
        // because the *same* minted state can be echoed with *different*
        // answers -- a client (or attacker) replaying one round-1 blob with two
        // different `inputResponses` would otherwise hit the first answer's
        // cached result for the second. Folding in the answers' digest keeps
        // those apart, while a genuine lost-response retry -- same state *and*
        // same answers -- still hits. Only committed *final* rounds are ever
        // cached, so a hit here is by construction a replay of one.
        #[cfg(not(feature = "legacy-spec"))]
        let state_tag: Option<String> = if mrtr_method {
            req.meta().and_then(|m| {
                let tag = m
                    .request_state
                    .as_deref()
                    .and_then(|blob| blob.rsplit_once('.').map(|(_, tag)| tag))?;
                let answers = m
                    .input_responses
                    .as_ref()
                    .map(crate::types::mrtr::state::input_responses_digest)
                    .unwrap_or_default();
                Some(format!("{tag}.{answers}"))
            })
        } else {
            None
        };
        // Claim the per-state reservation *before* the cache lookup and hold it
        // through the handler, commits and the final `put`. Two identical
        // final-round retries (e.g. a client that timed out and re-sent while
        // the first round is still committing) would otherwise both miss the
        // cache below and re-run the handler + `on_commit` effects. The loser
        // blocks here until the winner has cached, then hits it instead.
        //
        // NOTE: this guard MUST stay live until after the final `put` below.
        // Dropping it early (e.g. rewriting to `let _ = ...reserve().await;`)
        // releases the lock immediately and reopens the concurrent-retry race;
        // the explicit, self-describing name guards against that refactor.
        #[cfg(not(feature = "legacy-spec"))]
        let _reservation_guard_held_through_commit = match state_tag.as_deref() {
            Some(tag) => Some(options.request_state_store().reserve(tag).await),
            None => None,
        };
        #[cfg(not(feature = "legacy-spec"))]
        if let Some(tag) = state_tag.as_deref()
            && let Some(cached) = options.request_state_store().get(tag).await
        {
            options.complete_request(&full_id);
            let mut resp = cached.set_id(req_id.clone());
            if let Some(session_id) = session_id {
                resp = resp.set_session_id(session_id);
            }
            return resp;
        }

        #[cfg(feature = "tracing")]
        tracing::trace!(logger = "neva", "Received: {:?}", req);
        let resp = if let Some(handler) = handlers.get(&req.method) {
            tokio::select! {
            resp = handler.call(HandlerParams::Request(context, req)) => {
                options.complete_request(&full_id);
                resp
            }
            _ = token.cancelled() => {
                #[cfg(feature = "tracing")]
                tracing::debug!(
                    logger = "neva",
                    "The request with ID: {} has been cancelled", full_id);
                    Err(Error::from(ErrorCode::RequestCancelled))
                }
            }
        } else {
            Err(Error::from(ErrorCode::MethodNotFound))
        };

        // MRTR interception: if the handler requested input (recorded in the
        // shared `MrtrCtx`), convert to an `InputRequiredResult` regardless of
        // what the handler returned. The pending flag -- not the sentinel error
        // -- is the reliable signal, because tool/prompt/resource wrappers fold
        // a handler `Err` into an in-band error result before we see it.
        #[cfg(not(feature = "legacy-spec"))]
        let mut cache_final = false;
        #[cfg(not(feature = "legacy-spec"))]
        let resp = match (mrtr_method, mrtr_arc) {
            (true, Some(arc)) => {
                let has_pending = arc.pending.lock().map(|p| p.is_some()).unwrap_or(false);
                if has_pending {
                    build_input_required(
                        &arc,
                        &req_method,
                        &salient_params,
                        &options,
                        mrtr_principal,
                    )
                    .map(|ir| ir.into_response(req_id.clone()))
                } else if mrtr_should_commit(&resp) {
                    // Final round: run deferred commits in registration order.
                    // The first Err becomes the response error.
                    let commits = arc
                        .commits
                        .lock()
                        .map(|mut c| std::mem::take(&mut *c))
                        .unwrap_or_default();
                    let mut commit_err = None;
                    for fut in commits {
                        if let Err(e) = fut.await {
                            commit_err = Some(e);
                            break;
                        }
                    }
                    match commit_err {
                        Some(e) => Err(e),
                        None => {
                            // Final round committed successfully: cache its
                            // response (below, once the id is final) so a
                            // lost-response retry is served idempotently.
                            cache_final = true;
                            resp
                        }
                    }
                } else {
                    resp
                }
            }
            _ => resp,
        };

        let mut resp = resp.into_response(req_id);
        // The two things MCP 2026-07-28 puts on every result: the discriminator
        // and the server's own identity (`serverInfo` left `DiscoverResult`).
        // Both are stamped here, at the single seam every dispatched response
        // passes through -- a handler that returns a `Response` it built
        // elsewhere, or proxied from an upstream peer, reaches the wire without
        // passing `Response::success`.
        #[cfg(not(feature = "legacy-spec"))]
        {
            resp = resp
                .with_result_type()
                .with_server_info(&options.implementation);
        }
        if let Some(session_id) = session_id {
            resp = resp.set_session_id(session_id);
        }

        // Record the committed final response under the incoming state's tag
        // plus this round's answers digest (see `state_tag` above). Retained
        // until the state's own expiry window (`now + ttl`, an upper bound on
        // its remaining life), after which a retry is rejected as expired before
        // reaching the store.
        #[cfg(not(feature = "legacy-spec"))]
        if cache_final && let Some(tag) = state_tag.as_deref() {
            let exp = crate::types::mrtr::state::now_secs() + options.request_state_ttl_secs();
            options
                .request_state_store()
                .put(tag, resp.clone(), exp)
                .await;
        }
        resp
    }

    async fn handle_response(resp: Response, runtime: ServerRuntime) -> Response {
        let resp_id = resp.id().clone();
        let session_id = resp.session_id().cloned();

        // A suspended task `ctx.task().elicit` no longer resumes from an inbound
        // `Response`: MCP 2026-07-28 routes the answer through `tasks/update`,
        // which is a request handled by `App::update_task`. Responses arriving
        // here are ordinary replies to server-initiated requests and go to the
        // request queue unchanged.
        runtime.pending_requests().complete(resp);

        let mut resp = Response::empty(resp_id);
        if let Some(session_id) = session_id {
            resp = resp.set_session_id(session_id);
        }
        resp
    }

    #[inline]
    async fn handle_notification(notification: Notification, runtime: ServerRuntime) {
        match notification.method.as_str() {
            crate::types::notification::commands::CANCELLED => {
                if let Some(params) = notification.params
                    && let Ok(params) =
                        serde_json::from_value::<CancelledNotificationParams>(params)
                {
                    // A cancel aimed at a `subscriptions/listen` ends that
                    // subscription only. Cancelling the request itself would
                    // race the handler for its own response and rob the client
                    // of the graceful-close result.
                    #[cfg(not(feature = "legacy-spec"))]
                    if runtime.options().subscriptions().cancel(&params.request_id) {
                        return;
                    }
                    runtime.options().cancel_request(&params.request_id);
                }
            }
            crate::types::notification::commands::MESSAGE => {
                #[cfg(feature = "tracing")]
                notification.write();
            }
            _ => {}
        }
    }

    #[inline]
    fn wait_for_shutdown_signal(&mut self, token: CancellationToken) {
        shared::wait_for_shutdown_signal(token);
    }
}

/// Closes the per-request notification sink for a stateless HTTP `POST` when the
/// message's whole middleware pipeline is done (including on panic or task
/// cancellation, hence a `Drop` guard rather than a plain call).
///
/// Dropping the sender is the completion signal a request-scoped SSE `POST`
/// response waits for: only then does it know every notification -- including
/// ones user middleware emitted after `next(ctx)` -- has been queued, so it can
/// drain them and close with the final response.
#[cfg(all(not(feature = "legacy-spec"), feature = "http-server"))]
struct RequestSinkGuard(Option<uuid::Uuid>);

#[cfg(all(not(feature = "legacy-spec"), feature = "http-server"))]
impl Drop for RequestSinkGuard {
    fn drop(&mut self) {
        if let Some(id) = self.0 {
            crate::types::notification::sink::unregister(&id);
        }
    }
}

/// Builds the per-request tracing span carrying the session id.
///
/// See the 2026-07-28 variant below for why the span is created at `ERROR`.
#[cfg(all(feature = "tracing", feature = "legacy-spec"))]
fn create_tracing_span(session_id: Option<uuid::Uuid>) -> tracing::Span {
    if let Some(mcp_session_id) = session_id {
        tracing::error_span!("request", mcp_session_id = mcp_session_id.to_string())
    } else {
        tracing::error_span!("request")
    }
}

/// Builds the per-request tracing span, carrying the session id and (2026-07-28) the
/// request-scoped logging level as span fields the notification layer reads.
///
/// The span is created at `ERROR` -- the highest level -- so it is not itself
/// filtered out. It carries no message: it exists only to route and filter the
/// events emitted inside it. At a lower level a common global threshold (say
/// `LevelFilter::WARN`) would disable the span while still letting WARN/ERROR
/// events through, leaving those events with no `mcp_session_id` to route by and
/// no `mcp_log_level` to filter against -- request-scoped logging would silently
/// stop working. `ERROR` keeps the context observable for every event that the
/// application's own threshold admits.
#[cfg(all(feature = "tracing", not(feature = "legacy-spec")))]
fn create_tracing_span(
    session_id: Option<uuid::Uuid>,
    log_level: Option<crate::types::notification::LoggingLevel>,
) -> tracing::Span {
    match (session_id, log_level) {
        (Some(sid), Some(level)) => tracing::error_span!(
            "request",
            mcp_session_id = sid.to_string(),
            mcp_log_level = u64::from(level.severity())
        ),
        (Some(sid), None) => {
            tracing::error_span!("request", mcp_session_id = sid.to_string())
        }
        (None, Some(level)) => {
            tracing::error_span!("request", mcp_log_level = u64::from(level.severity()))
        }
        (None, None) => tracing::error_span!("request"),
    }
}

/// Returns a clone of `params` with the `_meta` key removed, so the MRTR
/// request-binding digest is stable across round-trips.
#[cfg(not(feature = "legacy-spec"))]
fn strip_meta(params: &serde_json::Value) -> serde_json::Value {
    match params {
        serde_json::Value::Object(map) => {
            let mut cloned = map.clone();
            cloned.remove("_meta");
            serde_json::Value::Object(cloned)
        }
        other => other.clone(),
    }
}

/// Why a `tools/call`'s mirrored `Mcp-Param-*` headers do not describe its
/// arguments, if they do not.
///
/// A tool may annotate arguments with `x-mcp-header`, and the client must then
/// mirror each present value into `Mcp-Param-{name}`. An intermediary is
/// entitled to route or rate-limit on those headers, so a header that says one
/// tenant while the body says another has to be rejected rather than
/// dispatched -- otherwise the annotation is a suggestion, not a control.
///
/// Each annotated argument is checked in both directions: a value in the body
/// requires the header, a header requires the value, and both present must
/// agree after decoding the Base64 sentinel. Headers naming an argument this
/// tool does not annotate are none of the origin server's business -- the spec
/// has unrecognized `Mcp-Param-*` forwarded and ignored.
///
/// A definition whose annotations are invalid yields no expectations at all:
/// the malformed tool is the problem, and the client is already required to
/// drop it rather than call it.
#[cfg(all(feature = "http-server", not(feature = "legacy-spec")))]
async fn param_header_error(
    req: &Request,
    options: &crate::app::options::RuntimeMcpOptions,
) -> Option<Error> {
    use crate::shared::param_headers;
    use crate::transport::http::decode_header_value;

    if req.method != crate::types::tool::commands::CALL {
        return None;
    }

    // Only a call that arrived as a single HTTP request has mirrored headers to
    // check. `Mcp-Method` is the marker: the transport preamble requires it on
    // every single request and rejects it on a batch, so its absence here means
    // this call came in a batch (or off another transport entirely) and no
    // header ever described it. Demanding one would reject every batched call
    // of an annotated tool.
    if !req.headers.contains_key(crate::transport::http::MCP_METHOD) {
        return None;
    }

    let params = req.params.as_ref()?.as_object()?;
    let name = params.get("name")?.as_str()?;
    let tool = options.get_tool(name).await?;
    let schema = serde_json::to_value(&tool.input_schema).ok()?;
    let declared = param_headers::collect(&schema).ok()?;
    if declared.is_empty() {
        return None;
    }

    let args = params.get("arguments").cloned().unwrap_or_default();
    let mirrored = param_headers::extract(&declared, &args);

    let mismatch = |header: &str, stated: &str, body: &str| {
        Some(Error::new(
            ErrorCode::HeaderMismatch,
            format!(
                "Header mismatch: {header} header value {stated:?} does not match body value {body:?}"
            ),
        ))
    };

    for header in &declared {
        let name = format!("{}{}", param_headers::PARAM_HEADER_PREFIX, header.header);
        let stated = req.headers.get(&name).and_then(|v| v.to_str().ok());
        let body = mirrored
            .iter()
            .find(|(mirrored, _)| *mirrored == name)
            .map(|(_, value)| value.as_str());

        match (stated, body) {
            (None, None) => {}
            (None, Some(body)) => {
                return Some(Error::new(
                    ErrorCode::HeaderMismatch,
                    format!("Missing {name} header for the mirrored argument {body:?}"),
                ));
            }
            (Some(stated), None) => {
                return Some(Error::new(
                    ErrorCode::HeaderMismatch,
                    format!(
                        "{name} header sent as {stated:?}, but the call carries no such argument"
                    ),
                ));
            }
            (Some(stated), Some(body)) => match decode_header_value(stated) {
                Some(decoded) if decoded == body => {}
                Some(decoded) => return mismatch(&name, &decoded, body),
                None => {
                    return Some(Error::new(
                        ErrorCode::HeaderMismatch,
                        format!("Malformed {name} header value"),
                    ));
                }
            },
        }
    }

    None
}

/// Decodes/verifies any incoming `requestState` and merges this round's
/// `inputResponses` into the replay log, producing the per-dispatch MRTR state.
#[cfg(not(feature = "legacy-spec"))]
fn seed_mrtr_ctx(
    req: &Request,
    method: &str,
    salient: &serde_json::Value,
    options: &crate::app::options::RuntimeMcpOptions,
    principal: Option<&str>,
) -> Result<std::sync::Arc<crate::app::context::MrtrCtx>, Error> {
    use crate::types::mrtr::state::{StateCodec, now_secs, request_binding};

    let meta = req.meta();
    let client_capabilities = meta
        .as_ref()
        .and_then(|m| m.client_capabilities)
        .unwrap_or_default();

    let mut answers = std::collections::HashMap::new();
    let mut memos = std::collections::HashMap::new();
    let mut effects = std::collections::HashSet::new();
    // Keys the server requested in the prior round, decoded from the verified
    // state. `None` means no valid state was supplied, so no input was solicited.
    let mut requested: Option<Vec<String>> = None;
    if let Some(state) = meta.as_ref().and_then(|m| m.request_state.clone()) {
        // Reject an oversized inbound state before decoding it. Base64 decoding
        // and AEAD decryption in `StateCodec::decode` both allocate/compute in
        // proportion to the blob size, so without this guard `with_max_state_bytes`
        // would only bound the states we *mint* and a bogus oversized
        // `requestState` from an untrusted client could force that work before
        // failing. The cap is the same encoded-length bound enforced on the
        // outbound path in `build_input_required`.
        if state.len() > options.max_state_bytes() {
            return Err(Error::new(
                ErrorCode::InvalidParams,
                "requestState exceeds the configured maximum size",
            ));
        }

        let payload = StateCodec::new(options.request_state_keys()).decode(&state)?;
        if payload.exp < now_secs() {
            return Err(Error::new(ErrorCode::InvalidParams, "requestState expired"));
        }

        if payload.req != request_binding(method, salient) {
            return Err(Error::new(
                ErrorCode::InvalidParams,
                "requestState does not match this request",
            ));
        }

        if payload.principal.as_deref() != principal {
            return Err(Error::new(
                ErrorCode::InvalidParams,
                "requestState principal mismatch",
            ));
        }

        answers = payload.answers;
        memos = payload.memos;
        effects = payload.effects;
        requested = Some(payload.requested);
    }
    if let Some(responses) = meta.and_then(|m| m.input_responses) {
        // `inputResponses` are answers to inputs the server requested in a
        // prior round; that request set lives in the encrypted `requestState`.
        // Without a verified state there is nothing to bind them to, so accept
        // only solicited, non-duplicate keys -- otherwise a client could
        // pre-seed answers for a later `ctx.elicit` key (skipping the intended
        // `InputRequiredResult`) or overwrite an already-resolved answer.
        let Some(requested) = requested.as_ref() else {
            return Err(Error::new(
                ErrorCode::InvalidParams,
                "inputResponses supplied without a requestState",
            ));
        };

        for (key, value) in responses {
            if answers.contains_key(&key) {
                return Err(Error::new(
                    ErrorCode::InvalidParams,
                    "inputResponses re-answers an already-resolved input",
                ));
            }
            if !requested.contains(&key) {
                return Err(Error::new(
                    ErrorCode::InvalidParams,
                    "inputResponses contains a key the server did not request",
                ));
            }
            answers.insert(key, value);
        }
    }

    Ok(std::sync::Arc::new(crate::app::context::MrtrCtx {
        answers,
        pending: Default::default(),
        client_capabilities,
        memos: std::sync::Mutex::new(memos),
        effects: std::sync::Mutex::new(effects),
        commits: Default::default(),
    }))
}

/// Builds the `InputRequiredResult` for the input the handler requested,
/// encoding a fresh encrypted `requestState`.
#[cfg(not(feature = "legacy-spec"))]
fn build_input_required(
    arc: &std::sync::Arc<crate::app::context::MrtrCtx>,
    method: &str,
    salient: &serde_json::Value,
    options: &crate::app::options::RuntimeMcpOptions,
    principal: Option<String>,
) -> Result<crate::types::mrtr::InputRequiredResult, Error> {
    use crate::types::mrtr::InputRequiredResult;
    use crate::types::mrtr::state::{StateCodec, StatePayload, now_secs, request_binding};

    let (key, request) = arc
        .pending
        .lock()
        .ok()
        .and_then(|mut p| p.take())
        .ok_or_else(|| Error::new(ErrorCode::InternalError, "missing pending MRTR input"))?;

    // Each kind is gated on its own flag: a client that fulfils elicitation
    // need not fulfil the deprecated sampling/roots kinds, and asking for one
    // it never declared would otherwise stall the round-trip.
    if !arc.client_capabilities.allows(&request) {
        return Err(Error::new(
            ErrorCode::MissingRequiredClientCapability,
            format!(
                "server requested `{}` but the client did not declare support",
                request.method()
            ),
        )
        .with_data(serde_json::json!({
            "requiredCapabilities": arc.client_capabilities.requiring(&request),
        })));
    }

    let memos = arc.memos.lock().map(|m| m.clone()).unwrap_or_default();
    let effects = arc.effects.lock().map(|e| e.clone()).unwrap_or_default();

    let payload = StatePayload {
        answers: arc.answers.clone(),
        // Bind the key we are requesting into the signed state so the next
        // round can verify the client only answers what we actually asked for.
        requested: vec![key.clone()],
        memos,
        effects,
        exp: now_secs() + options.request_state_ttl_secs(),
        req: request_binding(method, salient),
        principal,
    };

    let state = StateCodec::new(options.request_state_keys()).encode(&payload)?;
    if state.len() > options.max_state_bytes() {
        return Err(Error::new(
            ErrorCode::InternalError,
            "requestState too large",
        ));
    }

    Ok(InputRequiredResult::single(key, request, state))
}

/// Returns `true` only when `resp` is a genuine success that should trigger the
/// final round's deferred MRTR commits.
///
/// A protocol-level failure (`Err`, or an `Ok(Response::Err(..))`) is excluded
/// by construction. Crucially, so is an *in-band* tool error: tool/prompt
/// wrappers fold a handler `Err` into `Ok(CallToolResponse { isError: true })`,
/// so a plain `resp.is_ok()` check would still run commits on a failed call --
/// applying irreversible side effects (DB writes, charges) registered via
/// `ctx.on_commit(..)` even though the tool ultimately reported failure.
#[cfg(not(feature = "legacy-spec"))]
fn mrtr_should_commit(resp: &Result<Response, Error>) -> bool {
    match resp {
        Ok(Response::Ok(ok)) => ok.result.get("isError") != Some(&serde_json::Value::Bool(true)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::App;
    use crate::types::{MessageBatch, MessageEnvelope};

    #[test]
    fn it_enables_greeting_with_with_greeting() {
        let app = App::new().with_greeting();
        assert!(app.greeting);
    }

    /// The property names of a registered tool's `inputSchema`, sorted.
    fn schema_props(app: &App, tool: &str) -> Vec<String> {
        let tool = app.options.tools.as_ref().get(tool).expect("tool");
        #[cfg(feature = "legacy-spec")]
        let props = tool.input_schema.properties.as_ref().unwrap();
        #[cfg(not(feature = "legacy-spec"))]
        let props = tool.input_schema.as_value()["properties"]
            .as_object()
            .unwrap();

        let mut names = props.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }

    #[test]
    fn map_tool_macro_keeps_the_closures_parameter_names() {
        let mut app = App::new();
        crate::map_tool!(app, "greet", |name: String, age: i32| async move {
            format!("{name} is {age}")
        });

        assert_eq!(schema_props(&app, "greet"), ["age", "name"]);

        let names = &app.options.tools.as_ref().get("greet").unwrap().arg_names;
        assert!(names.is_declared());
        assert_eq!(names.get(0), "name");
        assert_eq!(names.get(1), "age");
    }

    #[test]
    fn map_tool_macro_skips_metadata_parameters() {
        use crate::types::{Meta, ProgressToken};

        let mut app = App::new();
        crate::map_tool!(
            app,
            "greet",
            |token: Meta<ProgressToken>, name: String| async move {
                let _ = token;
                name
            }
        );

        // `token` is served from `_meta`: it is neither published nor named,
        // so `name` is still the first argument slot.
        assert_eq!(schema_props(&app, "greet"), ["name"]);
        assert_eq!(
            app.options
                .tools
                .as_ref()
                .get("greet")
                .unwrap()
                .arg_names
                .get(0),
            "name"
        );
    }

    #[test]
    fn map_tool_macro_names_an_optional_argument() {
        let mut app = App::new();
        crate::map_tool!(app, "greet", |name: String, age: Option<i32>| async move {
            format!("{name} {age:?}")
        });

        // An `Option<T>` parameter is an argument: it is named, published and
        // occupies a slot -- it just need not be supplied.
        assert_eq!(schema_props(&app, "greet"), ["age", "name"]);

        let names = &app.options.tools.as_ref().get("greet").unwrap().arg_names;
        assert_eq!(names.arity(), 2);
        assert_eq!(names.get(1), "age");
    }

    #[tokio::test]
    async fn map_prompt_macro_keeps_names_and_optionality() {
        use crate::types::Role;

        let mut app = App::new();
        crate::map_prompt!(
            app,
            "analyze",
            |ctx: crate::Context, lang: String, tone: Option<String>| async move {
                let _ = ctx;
                (format!("{lang} {tone:?}"), Role::User)
            }
        );

        let prompt = app.options.prompts.as_ref().get("analyze").expect("prompt");
        let args = prompt.args.as_ref().unwrap();

        // `ctx` is served from the request, so it is neither published nor
        // named; `tone` is published as an argument peers may leave out.
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].name, "lang");
        assert_eq!(args[0].required, Some(true));
        assert_eq!(args[1].name, "tone");
        assert_eq!(args[1].required, Some(false));
        assert_eq!(prompt.arg_names.get(0), "lang");
        assert_eq!(prompt.arg_names.get(1), "tone");
    }

    #[test]
    fn a_bare_closure_publishes_the_names_it_reads() {
        let mut app = App::new();
        app.map_tool("greet", |name: String, age: i32| async move {
            format!("{name} is {age}")
        });

        let tool = app.options.tools.as_ref().get("greet").unwrap();

        assert!(!tool.arg_names.is_declared());
        assert_eq!(tool.arg_names.arity(), 2);
        assert_eq!(schema_props(&app, "greet"), ["arg0", "arg1"]);
    }

    /// A hand-written schema with a single required `name` string property,
    /// spelled for whichever schema model the build uses.
    fn name_schema() -> crate::types::ToolInputSchema {
        const JSON: &str = r#"{"type":"object","properties":{"name":{"type":"string"}}}"#;
        #[cfg(feature = "legacy-spec")]
        {
            crate::types::tool::ToolSchema::from_json_str(JSON)
        }
        #[cfg(not(feature = "legacy-spec"))]
        {
            crate::types::schema_2020::InputSchema::from_json_str(JSON).unwrap_or_default()
        }
    }

    #[test]
    #[should_panic(expected = "publishes an inputSchema without the argument `arg0`")]
    fn startup_rejects_a_schema_the_handler_cannot_read() {
        let mut app = App::new();
        app.map_tool("greet", |name: String| async move { name })
            .with_input_schema(|_| name_schema());

        app.validate_arg_names();
    }

    #[test]
    #[should_panic(expected = "declares 1 argument name(s) but its handler takes 2")]
    fn startup_rejects_a_miscounted_declaration() {
        let mut app = App::new();
        app.map_tool("greet", |name: String, age: i32| async move {
            format!("{name} is {age}")
        })
        .with_arg_names(["name"]);

        app.validate_arg_names();
    }

    #[test]
    #[should_panic(expected = "declares the argument `q` but publishes an inputSchema without it")]
    fn startup_rejects_names_the_schema_does_not_offer() {
        let mut app = App::new();
        app.map_tool("search", |q: String| async move { q })
            .with_input_schema(|_| {
                const JSON: &str = r#"{"type":"object","properties":{"query":{"type":"string"}}}"#;
                #[cfg(feature = "legacy-spec")]
                {
                    crate::types::tool::ToolSchema::from_json_str(JSON)
                }
                #[cfg(not(feature = "legacy-spec"))]
                {
                    crate::types::schema_2020::InputSchema::from_json_str(JSON).unwrap_or_default()
                }
            })
            .with_arg_names(["q"]);

        app.validate_arg_names();
    }

    /// A schema that describes its arguments through `$ref` or a composition
    /// keyword has no top-level `properties` to check against, and is left
    /// alone rather than failed on a guess.
    #[test]
    fn startup_accepts_a_schema_without_top_level_properties() {
        let mut app = App::new();
        app.map_tool("search", |q: String| async move { q })
            .with_input_schema(|_| {
                const JSON: &str = r##"{"$ref": "#/$defs/Args"}"##;
                #[cfg(feature = "legacy-spec")]
                {
                    crate::types::tool::ToolSchema::from_json_str(JSON)
                }
                #[cfg(not(feature = "legacy-spec"))]
                {
                    crate::types::schema_2020::InputSchema::from_json_str(JSON).unwrap_or_default()
                }
            })
            .with_arg_names(["q"]);

        app.validate_arg_names();
    }

    /// A schema may describe some arguments in its top-level `properties` and
    /// the rest through composition. The map alone is then not the whole
    /// story, and reading it as if it were would fail a well-formed tool.
    #[test]
    #[cfg(not(feature = "legacy-spec"))]
    fn startup_accepts_a_schema_with_composed_properties() {
        let mut app = App::new();
        app.map_tool("search", |q: String, page: i32| async move {
            format!("{q}{page}")
        })
        .with_input_schema(|_| {
            crate::types::schema_2020::InputSchema::from_json_str(
                r#"{
                    "type": "object",
                    "properties": { "q": { "type": "string" } },
                    "allOf": [
                        { "properties": { "page": { "type": "number" } } }
                    ]
                }"#,
            )
            .unwrap_or_default()
        })
        .with_arg_names(["q", "page"]);

        app.validate_arg_names();
    }

    #[test]
    #[should_panic(expected = "prompt `analyze` publishes 1 argument(s) but its handler takes 2")]
    fn startup_rejects_a_prompt_that_publishes_too_few_args() {
        use crate::types::Role;

        let mut app = App::new();
        app.map_prompt("analyze", |topic: String, tone: String| async move {
            (format!("{topic}{tone}"), Role::User)
        })
        .with_args(["topic"]);

        app.validate_arg_names();
    }

    #[test]
    fn startup_accepts_a_prompt_that_publishes_every_arg() {
        use crate::types::Role;

        let mut app = App::new();
        app.map_prompt("analyze", |topic: String, tone: String| async move {
            (format!("{topic}{tone}"), Role::User)
        })
        .with_args(["topic", "tone"]);

        let prompt = app.options.prompts.as_ref().get("analyze").unwrap();
        assert_eq!(prompt.arg_names.arity(), 2);
        assert_eq!(prompt.arg_names.get(1), "tone");

        app.validate_arg_names();
    }

    #[test]
    fn startup_accepts_a_declared_tool() {
        let mut app = App::new();
        app.map_tool("greet", |name: String| async move { name })
            .with_input_schema(|_| name_schema())
            .with_arg_names(["name"]);

        app.validate_arg_names();
    }

    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn with_max_state_bytes_sets_the_option() {
        let app = App::new().with_max_state_bytes(4096);
        assert_eq!(app.options.max_state_bytes(), 4096);
    }

    /// The request span carries the routing and filtering context for every
    /// event emitted while handling a request, so a common global threshold
    /// (`LevelFilter::WARN`) must not disable it: WARN/ERROR events stay enabled
    /// and would otherwise lose their `mcp_session_id` and `mcp_log_level`.
    #[cfg(all(feature = "tracing", not(feature = "legacy-spec")))]
    #[test]
    fn request_span_survives_a_warning_only_filter() {
        use crate::types::notification::LoggingLevel;
        use tracing_subscriber::prelude::*;

        let subscriber =
            tracing_subscriber::registry().with(tracing::level_filters::LevelFilter::WARN);

        tracing::subscriber::with_default(subscriber, || {
            let span =
                super::create_tracing_span(Some(uuid::Uuid::new_v4()), Some(LoggingLevel::Warning));
            assert!(
                !span.is_disabled(),
                "the request span must stay enabled under a warning-only filter"
            );
            // The contrast: an INFO span -- what this used to be -- is dropped,
            // taking the request context with it.
            assert!(tracing::info_span!("request").is_disabled());
        });
    }

    /// Deferred MRTR commits must run only for a genuine success -- never for a
    /// protocol-level error nor for an in-band tool error (`isError: true`),
    /// which tool wrappers fold a handler `Err` into.
    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn mrtr_should_commit_excludes_errors() {
        use crate::error::Error;
        use crate::types::{RequestId, Response};
        use serde_json::json;

        let id = RequestId::Number(1);

        // Genuine success -> commit.
        let ok = Ok(Response::success(
            id.clone(),
            json!({ "content": [], "isError": false }),
        ));
        assert!(super::mrtr_should_commit(&ok));

        // Success with no `isError` field at all (e.g. a non-tool result) -> commit.
        let plain = Ok(Response::success(id.clone(), json!({ "ok": true })));
        assert!(super::mrtr_should_commit(&plain));

        // In-band tool error folded into Ok -> do NOT commit.
        let tool_err = Ok(Response::success(
            id.clone(),
            json!({ "content": [], "isError": true }),
        ));
        assert!(!super::mrtr_should_commit(&tool_err));

        // Protocol-level error response -> do NOT commit.
        let proto_err = Ok(Response::error(id.clone(), Error::new(-32603, "boom")));
        assert!(!super::mrtr_should_commit(&proto_err));

        // Handler `Err` -> do NOT commit.
        let hard_err: Result<Response, Error> = Err(Error::new(-32603, "boom"));
        assert!(!super::mrtr_should_commit(&hard_err));
    }

    /// Security guards in [`super::seed_mrtr_ctx`] that the e2e happy path never
    /// exercises: an expired `requestState` and a principal-bound state replayed
    /// under a different principal. Driven deterministically (no clock advance,
    /// no auth harness) by hand-encoding the signed blob.
    #[cfg(not(feature = "legacy-spec"))]
    mod mrtr_seed_guards {
        use crate::app::App;
        use crate::error::ErrorCode;
        use crate::types::mrtr::state::{StateCodec, StatePayload, now_secs, request_binding};
        use crate::types::{Request, RequestId};

        const SECRET: &[u8] = b"unit-secret";
        const METHOD: &str = "tools/call";

        fn options() -> crate::app::options::RuntimeMcpOptions {
            App::new()
                .with_request_state_secret(SECRET)
                .options
                .into_runtime()
        }

        fn salient() -> serde_json::Value {
            serde_json::json!({ "name": "greet", "arguments": {} })
        }

        fn request_with_state(state: &str) -> Request {
            let mut params = salient();
            params["_meta"] = serde_json::json!({
                "requestState": state,
                "io.modelcontextprotocol/clientCapabilities": { "elicitation": true }
            });
            Request::new(Some(RequestId::Number(1)), METHOD, Some(params))
        }

        fn encode(payload: &StatePayload) -> String {
            let ring = crate::types::mrtr::state::StateKeyring::single(SECRET);
            StateCodec::new(&ring).encode(payload).expect("encode")
        }

        #[test]
        fn expired_request_state_is_rejected() {
            let payload = StatePayload {
                answers: Default::default(),
                requested: Default::default(),
                memos: Default::default(),
                effects: Default::default(),
                exp: now_secs().saturating_sub(1), // already in the past
                req: request_binding(METHOD, &salient()),
                principal: None,
            };
            let req = request_with_state(&encode(&payload));
            let err = super::super::seed_mrtr_ctx(&req, METHOD, &salient(), &options(), None)
                .expect_err("expired state must be rejected");
            assert_eq!(err.code, ErrorCode::InvalidParams);
            assert!(format!("{err}").contains("expired"), "{err}");
        }

        #[test]
        fn principal_mismatch_is_rejected() {
            // State minted for "alice" (valid, unexpired, correctly bound)...
            let payload = StatePayload {
                answers: Default::default(),
                requested: Default::default(),
                memos: Default::default(),
                effects: Default::default(),
                exp: now_secs() + 300,
                req: request_binding(METHOD, &salient()),
                principal: Some("alice".into()),
            };
            let req = request_with_state(&encode(&payload));
            // ...replayed by "bob".
            let err =
                super::super::seed_mrtr_ctx(&req, METHOD, &salient(), &options(), Some("bob"))
                    .expect_err("principal mismatch must be rejected");
            assert_eq!(err.code, ErrorCode::InvalidParams);
            assert!(format!("{err}").contains("principal mismatch"), "{err}");
        }

        /// Builds the `_meta` JSON with the given request state (if any) and
        /// `inputResponses` map of key -> accepted [`ElicitResult`].
        fn request_with_responses(state: Option<&str>, response_keys: &[&str]) -> Request {
            use crate::types::elicitation::ElicitResult;
            let responses: serde_json::Map<String, serde_json::Value> = response_keys
                .iter()
                .map(|k| {
                    (
                        (*k).to_owned(),
                        serde_json::to_value(ElicitResult::accept()).expect("serialize result"),
                    )
                })
                .collect();
            let mut meta = serde_json::json!({
                "io.modelcontextprotocol/clientCapabilities": { "elicitation": true },
                "inputResponses": responses,
            });
            if let Some(state) = state {
                meta["requestState"] = serde_json::json!(state);
            }
            let mut params = salient();
            params["_meta"] = meta;
            Request::new(Some(RequestId::Number(1)), METHOD, Some(params))
        }

        fn state_with(answers: &[&str], requested: &[&str]) -> String {
            use crate::types::elicitation::ElicitResult;
            let answers = answers
                .iter()
                .filter_map(|k| {
                    Some((
                        (*k).to_owned(),
                        serde_json::to_value(ElicitResult::accept()).ok()?,
                    ))
                })
                .collect();
            let payload = StatePayload {
                answers,
                requested: requested.iter().map(|k| (*k).to_owned()).collect(),
                memos: Default::default(),
                effects: Default::default(),
                exp: now_secs() + 300,
                req: request_binding(METHOD, &salient()),
                principal: None,
            };
            encode(&payload)
        }

        #[test]
        fn input_responses_without_request_state_are_rejected() {
            // No validated state: nothing solicited these answers.
            let req = request_with_responses(None, &["ask_name"]);
            let err = super::super::seed_mrtr_ctx(&req, METHOD, &salient(), &options(), None)
                .expect_err("unbound inputResponses must be rejected");
            assert_eq!(err.code, ErrorCode::InvalidParams);
            assert!(format!("{err}").contains("without a requestState"), "{err}");
        }

        #[test]
        fn unsolicited_input_response_key_is_rejected() {
            // State requested `ask_name`; client answers an unrelated key.
            let state = state_with(&[], &["ask_name"]);
            let req = request_with_responses(Some(&state), &["ask_age"]);
            let err = super::super::seed_mrtr_ctx(&req, METHOD, &salient(), &options(), None)
                .expect_err("unsolicited key must be rejected");
            assert_eq!(err.code, ErrorCode::InvalidParams);
            assert!(format!("{err}").contains("did not request"), "{err}");
        }

        #[test]
        fn re_answering_a_resolved_input_is_rejected() {
            // `ask_name` already resolved in the signed answers log; the client
            // tries to overwrite it (even though it is also in `requested`).
            let state = state_with(&["ask_name"], &["ask_name"]);
            let req = request_with_responses(Some(&state), &["ask_name"]);
            let err = super::super::seed_mrtr_ctx(&req, METHOD, &salient(), &options(), None)
                .expect_err("re-answering must be rejected");
            assert_eq!(err.code, ErrorCode::InvalidParams);
            assert!(format!("{err}").contains("already-resolved"), "{err}");
        }

        #[test]
        fn solicited_input_response_is_accepted() {
            // The happy path: client answers exactly the requested key.
            let state = state_with(&[], &["ask_name"]);
            let req = request_with_responses(Some(&state), &["ask_name"]);
            let ctx = super::super::seed_mrtr_ctx(&req, METHOD, &salient(), &options(), None)
                .expect("solicited response must be accepted");
            assert!(ctx.answers.contains_key("ask_name"));
        }
    }

    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn registers_discover_not_initialize() {
        let app = App::new();
        assert!(app.handlers.contains_key(crate::commands::DISCOVER));
        assert!(!app.handlers.contains_key(crate::commands::INIT));
    }

    #[cfg(feature = "legacy-spec")]
    #[test]
    fn default_registers_initialize() {
        let app = App::new();
        assert!(app.handlers.contains_key(crate::commands::INIT));
    }

    #[test]
    fn it_disables_greeting_with_without_greeting() {
        let app = App::new().without_greeting();
        assert!(!app.greeting);
    }

    #[test]
    fn batch_filtering_notifications_yield_no_response_slots() {
        use crate::types::notification::Notification;

        // Build a notification-only batch
        let batch = MessageBatch::new(vec![
            MessageEnvelope::Notification(Notification::new("notifications/foo", None)),
            MessageEnvelope::Notification(Notification::new("notifications/bar", None)),
        ])
        .expect("non-empty batch must be constructable");

        // Replicate the filter logic from execute_batch:
        // Request -> Some(response slot), Notification/Response -> None
        let response_slots: Vec<MessageEnvelope> = batch
            .into_iter()
            .filter_map(|envelope| match envelope {
                MessageEnvelope::Request(_) => Some(envelope),
                _ => None,
            })
            .collect();

        assert!(
            response_slots.is_empty(),
            "notification-only batch must produce zero response slots"
        );
    }

    #[test]
    fn batch_filtering_requests_yield_response_slots() {
        use crate::types::{Request, RequestId};

        // Build a request-only batch
        let req1 = Request::new(Some(RequestId::Number(1)), "tools/list", None::<()>);
        let req2 = Request::new(Some(RequestId::Number(2)), "ping", None::<()>);
        let batch = MessageBatch::new(vec![
            MessageEnvelope::Request(req1),
            MessageEnvelope::Request(req2),
        ])
        .expect("non-empty batch must be constructable");

        // Replicate the filter: only Request envelopes produce response slots
        let response_slots: Vec<MessageEnvelope> = batch
            .into_iter()
            .filter_map(|envelope| match envelope {
                MessageEnvelope::Request(_) => Some(envelope),
                _ => None,
            })
            .collect();

        assert_eq!(
            response_slots.len(),
            2,
            "two requests must produce two response slots"
        );
    }
}

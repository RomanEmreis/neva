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
use crate::shared;
#[cfg(not(feature = "legacy-spec"))]
use crate::types::{RequestId, SubscriptionsListenRequestParams, SubscriptionsListenResult};
#[cfg(feature = "legacy-spec")]
use crate::types::{SubscribeRequestParams, UnsubscribeRequestParams};
#[cfg(not(feature = "legacy-spec"))]
use tokio_util::sync::CancellationToken;

#[cfg(not(feature = "legacy-spec"))]
use self::shutdown::DEFAULT_SHUTDOWN_DRAIN;

#[cfg(not(feature = "legacy-spec"))]
use self::mrtr::{build_input_required, mrtr_should_commit, salient_params, seed_mrtr_ctx};

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
mod commands;
pub mod context;
mod dispatch;
#[cfg(not(feature = "legacy-spec"))]
pub mod extension;
mod greeter;
pub(crate) mod handler;
#[cfg(not(feature = "legacy-spec"))]
mod mrtr;
#[cfg(not(feature = "legacy-spec"))]
pub mod mrtr_store;
#[cfg(not(feature = "legacy-spec"))]
pub mod notification_bus;
pub mod options;
pub mod shutdown;
#[cfg(not(feature = "legacy-spec"))]
pub(crate) mod subscriptions;

pub use shutdown::ShutdownHandle;

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

    /// What a shutdown request arrives on, whether from an OS signal or from a
    /// [`ShutdownHandle`] the caller kept.
    shutdown: ShutdownHandle,

    /// Ceiling on the wait for live subscriptions to answer before the
    /// transport is torn down. See [`DEFAULT_SHUTDOWN_DRAIN`].
    #[cfg(not(feature = "legacy-spec"))]
    shutdown_drain: std::time::Duration,
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
            shutdown: ShutdownHandle::new(),
            #[cfg(not(feature = "legacy-spec"))]
            shutdown_drain: DEFAULT_SHUTDOWN_DRAIN,
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

    /// Takes a handle that stops this server without an OS signal.
    ///
    /// The handle composes with the signal handler rather than replacing it:
    /// whichever fires first shuts the server down, so a server built this way
    /// still stops on Ctrl+C.
    ///
    /// Await [`run`](Self::run) to know the server actually finished: this
    /// only requests the stop.
    #[cfg_attr(
        not(feature = "legacy-spec"),
        doc = "
Under MCP 2026-07-28 that gap is where live `subscriptions/listen` streams get
answered, bounded by [`with_shutdown_drain`](Self::with_shutdown_drain)."
    )]
    ///
    /// # Example
    /// ```no_run
    /// use neva::App;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let (app, shutdown) = App::new().with_shutdown();
    /// let server = tokio::spawn(app.run());
    ///
    /// shutdown.shutdown();
    /// server.await.expect("the server task panicked");
    /// # }
    /// ```
    pub fn with_shutdown(self) -> (Self, ShutdownHandle) {
        let handle = self.shutdown.clone();
        (self, handle)
    }

    /// Stops this server on a [`ShutdownHandle`] the caller already has --
    /// one signal shared with the rest of a larger service, rather than one
    /// this server hands out.
    ///
    /// Composes with the OS signal handler exactly as
    /// [`with_shutdown`](Self::with_shutdown) does.
    ///
    /// # Example
    /// ```no_run
    /// use neva::{App, app::ShutdownHandle};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let shutdown = ShutdownHandle::new();
    /// let app = App::new().with_shutdown_signal(shutdown.clone());
    ///
    /// let server = tokio::spawn(app.run());
    /// shutdown.shutdown();
    /// server.await.expect("the server task panicked");
    /// # }
    /// ```
    pub fn with_shutdown_signal(mut self, handle: ShutdownHandle) -> Self {
        self.shutdown = handle;
        self
    }

    /// Caps how long shutdown waits for live `subscriptions/listen` streams to
    /// answer before the transport is torn down anyway.
    ///
    /// This is a ceiling, not a delay: the wait ends the moment the last
    /// result is queued, and is skipped outright when no subscription is open.
    /// Raise it for a server whose subscriptions have deep buffers to flush;
    /// `Duration::ZERO` opts out and restores an abrupt close.
    ///
    /// Default: 2 seconds.
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(not(feature = "legacy-spec"))] {
    /// use neva::App;
    /// use std::time::Duration;
    ///
    /// let app = App::new()
    ///     .with_shutdown_drain(Duration::from_secs(5));
    /// # }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub fn with_shutdown_drain(mut self, drain: std::time::Duration) -> Self {
        self.shutdown_drain = drain;
        self
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

        // Read before `self.options` moves into the runtime below.
        let greeted = self.greeting;

        let mut transport = self.options.transport();
        let cancellation_token = transport.start();

        // Shutdown arrives here -- from an OS signal, or from a
        // `ShutdownHandle` the caller kept -- and is relayed to the transport
        // rather than being the transport's own token. What happens in between
        // is the drain, below.
        let shutdown = self.shutdown.token();
        self.wait_for_shutdown_signal(shutdown.clone());

        // Long-lived requests (`subscriptions/listen`) end one phase ahead of
        // the transport: they watch a token of their own, so their
        // graceful-close results are produced while the writers are still
        // reading. Sharing one token is what made those results race a channel
        // whose reader had already gone.
        #[cfg(not(feature = "legacy-spec"))]
        let subscriptions_token = CancellationToken::new();
        #[cfg(not(feature = "legacy-spec"))]
        self.options.set_shutdown_token(subscriptions_token.clone());

        #[cfg(not(feature = "legacy-spec"))]
        Self::relay_shutdown(
            shutdown,
            subscriptions_token,
            cancellation_token.clone(),
            self.options.subscriptions().clone(),
            self.options.in_flight(),
            self.shutdown_drain,
        );
        #[cfg(feature = "legacy-spec")]
        Self::relay_shutdown(shutdown, cancellation_token.clone());

        // With a notification bus installed, every subscribable notification --
        // this instance's own included -- comes back through the bus, and this
        // task is what turns it into a delivery to the subscribers this
        // instance holds. Subscribing happens here rather than inside the
        // spawned task so the stream exists before the transport accepts its
        // first request.
        #[cfg(not(feature = "legacy-spec"))]
        if let Some(bus) = self.options.notification_bus() {
            let mut stream = bus.subscribe();
            let subscriptions = self.options.subscriptions().clone();
            let token = cancellation_token.clone();

            tokio::spawn(async move {
                use futures_util::StreamExt;
                loop {
                    tokio::select! {
                        biased;
                        _ = token.cancelled() => break,
                        next = stream.next() => match next {
                            Some(notification) => {
                                subscriptions
                                    .broadcast(notification.method(), notification.params());
                            }
                            // A bus that ends its stream stops delivery for
                            // good; an implementation able to reconnect is
                            // expected to do so without ending it.
                            None => {
                                #[cfg(feature = "tracing")]
                                tracing::warn!(
                                    logger = "neva",
                                    "the notification bus ended its stream: cross-instance \
                                     subscription notifications will no longer be delivered"
                                );
                                break;
                            }
                        },
                    }
                }
            });
        }

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
                        Ok(msg) => {
                            // Taken here, on the accepting task, and not inside
                            // the spawned future: `tokio::spawn` only queues
                            // that future, so a guard created in it does not
                            // exist until the runtime first polls it. Shutdown
                            // landing in between would see no listen owed and
                            // skip the drain for one it had already accepted.
                            #[cfg(not(feature = "legacy-spec"))]
                            let arriving = dispatch::opens_subscription(&msg)
                                .then(|| runtime.options().subscriptions().arriving());

                            let runtime = runtime.clone();
                            match msg {
                                Message::Batch(batch) => {
                                    tokio::spawn(async move {
                                        #[cfg(not(feature = "legacy-spec"))]
                                        let _arriving = arriving;
                                        Self::execute_batch(batch, runtime).await;
                                    });
                                },
                                msg => {
                                    tokio::spawn(async move {
                                        #[cfg(not(feature = "legacy-spec"))]
                                        let _arriving = arriving;
                                        Self::execute(msg, runtime).await;
                                    });
                                }
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

        // Closes what the banner opened. Only for a greeted server: one that
        // asked for no banner asked for a quiet terminal.
        if greeted {
            greeter::print_farewell(std::env::var_os("NO_COLOR").is_none());
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

    /// Binds MRTR `requestState` to this service's identity, so a state minted
    /// for one service cannot be replayed against another
    /// (MCP 2026-07-28).
    ///
    /// The sealed state already carries a binding to the request that produced
    /// it and to the authenticated principal, but neither says which *service*
    /// minted it. That only matters where more than one can decrypt it -- which
    /// is exactly what a fleet sharing one
    /// [`with_request_state_secret`](Self::with_request_state_secret) is: give
    /// two services a method and parameters they both serve, and a state minted
    /// by one is a state the other accepts, mid-flow, with its answers already
    /// sealed in.
    ///
    /// Set it to whatever names this service and nothing else -- its canonical
    /// resource URI, the value `OAuthResourceOptions::with_resource` carries,
    /// is the natural one (link omitted: that type needs the `server-oauth`
    /// feature). It has to be **identical on every instance of the same
    /// service**, since a retry may land on any of them; a value that varies
    /// per instance rejects every retry that moves.
    ///
    /// Unset, states are minted and demanded without an audience -- and a state
    /// carrying one is refused just as firmly, so the guard cannot be shed by
    /// omitting it.
    ///
    /// An audience-bound state is also sealed under its own wire version, which
    /// a binary predating this option refuses outright. Without that, such a
    /// binary would decrypt the state, ignore the field it does not know, and
    /// run the round -- so the binding would be worth nothing against exactly
    /// the service that has not been upgraded yet. The cost is that states in
    /// flight when the option is turned on are rejected; they lapse within the
    /// `requestState` TTL (5 minutes), and the client's next round mints one
    /// under the new binding.
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(not(feature = "legacy-spec"))] {
    /// use neva::App;
    ///
    /// let app = App::new()
    ///     .with_request_state_secret(b"shared-secret")
    ///     .with_request_state_audience("https://weather.example.com/mcp");
    /// # }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub fn with_request_state_audience(mut self, audience: impl AsRef<str>) -> Self {
        self.options.set_request_state_audience(audience.as_ref());
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

    /// Sets the bus that carries subscription notifications between instances
    /// of this server (MCP 2026-07-28).
    ///
    /// A `subscriptions/listen` stream is a socket held open by one process,
    /// and the stateless transport pins nothing to an instance, so the
    /// subscriber and the request that mutates the server routinely land on
    /// different ones. With a bus installed, a notification produced anywhere
    /// reaches the subscribers everywhere: each instance publishes what it
    /// produces and delivers what it receives to the streams it holds.
    ///
    /// Defaults to no bus at all -- notifications go straight to this
    /// instance's own subscribers, which is what a single-instance server
    /// wants and costs nothing. **Multi-instance deployments serving
    /// subscriptions should set one** (e.g. Redis pub/sub), the same
    /// constraint as
    /// [`with_request_state_store`](Self::with_request_state_store).
    ///
    /// Read [`NotificationBus`](crate::app::notification_bus::NotificationBus)
    /// before implementing one: the contract requires that `subscribe` echo
    /// this instance's own publishes back to it.
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(not(feature = "legacy-spec"))] {
    /// # use neva::app::notification_bus::{BusNotification, NotificationBus};
    /// # use neva::shared::Stream;
    /// # struct RedisBus;
    /// # impl NotificationBus for RedisBus {
    /// #     async fn publish(&self, _: BusNotification) {}
    /// #     fn subscribe(&self) -> impl Stream<Item = BusNotification> + Send + 'static {
    /// #         futures_util::stream::empty()
    /// #     }
    /// # }
    /// use neva::App;
    ///
    /// let app = App::new()
    ///     .with_notification_bus(RedisBus);
    /// # }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub fn with_notification_bus(
        mut self,
        bus: impl crate::app::notification_bus::NotificationBus + 'static,
    ) -> Self {
        self.options.set_notification_bus(std::sync::Arc::new(bus));
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

    /// Zero is a count like any other: a name handed to a handler that reads
    /// none is the same miscount as too few names for one that reads several.
    #[test]
    #[should_panic(expected = "declares 1 argument name(s) but its handler takes 0")]
    fn startup_rejects_a_declaration_on_a_zero_arity_handler() {
        let mut app = App::new();
        app.map_tool("ping", || async { "pong" })
            .with_arg_names(["value"]);

        app.validate_arg_names();
    }

    /// The counterpart: naming nothing for a handler that reads nothing is a
    /// declaration that agrees with its handler, and is left alone.
    #[test]
    fn startup_accepts_an_empty_declaration_on_a_zero_arity_handler() {
        let mut app = App::new();
        app.map_tool("ping", || async { "pong" })
            .with_arg_names(Vec::<String>::new());

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

    /// A schema may name an argument by pattern rather than list it, and the
    /// top-level map is then not the whole story either.
    #[test]
    #[cfg(not(feature = "legacy-spec"))]
    fn startup_accepts_a_schema_with_pattern_properties() {
        let mut app = App::new();
        app.map_tool("search", |q: String, page: i32| async move {
            format!("{q}{page}")
        })
        .with_input_schema(|_| {
            crate::types::schema_2020::InputSchema::from_json_str(
                r#"{
                    "type": "object",
                    "properties": { "page": { "type": "number" } },
                    "patternProperties": { "^q$": { "type": "string" } },
                    "required": ["page", "q"]
                }"#,
            )
            .unwrap_or_default()
        })
        .with_arg_names(["q", "page"]);

        app.validate_arg_names();
    }

    /// Permitting further names is not the same as naming one. A schema left
    /// open by `additionalProperties` still tells no peer to send `q`, so the
    /// tool is as uncallable as with a closed schema and is still reported.
    #[test]
    #[should_panic(expected = "declares the argument `q` but publishes an inputSchema without it")]
    #[cfg(not(feature = "legacy-spec"))]
    fn startup_checks_a_schema_left_open_to_further_properties() {
        let mut app = App::new();
        app.map_tool("search", |q: String| async move { q })
            .with_input_schema(|_| {
                crate::types::schema_2020::InputSchema::from_json_str(
                    r#"{"type":"object","properties":{},"additionalProperties":{"type":"string"}}"#,
                )
                .unwrap_or_default()
            })
            .with_arg_names(["q"]);

        app.validate_arg_names();
    }

    /// `propertyNames` constrains what names may appear; it declares none. It
    /// is no reason to stop checking, least of all next to an
    /// `additionalProperties: false` that closes the schema outright.
    #[test]
    #[should_panic(expected = "declares the argument `q` but publishes an inputSchema without it")]
    #[cfg(not(feature = "legacy-spec"))]
    fn startup_checks_a_schema_constraining_property_names() {
        let mut app = App::new();
        app.map_tool("search", |q: String| async move { q })
            .with_input_schema(|_| {
                crate::types::schema_2020::InputSchema::from_json_str(
                    r#"{
                        "type": "object",
                        "properties": { "query": { "type": "string" } },
                        "propertyNames": { "pattern": "^[a-z]+$" },
                        "additionalProperties": false
                    }"#,
                )
                .unwrap_or_default()
            })
            .with_arg_names(["q"]);

        app.validate_arg_names();
    }

    /// `not` composes the other way round: it says what an instance must fail.
    /// A name under it is one no peer may send, so it cannot be the one that
    /// makes the top-level map incomplete, and the check still applies.
    #[test]
    #[should_panic(expected = "declares the argument `q` but publishes an inputSchema without it")]
    #[cfg(not(feature = "legacy-spec"))]
    fn startup_checks_a_schema_whose_only_composition_is_not() {
        let mut app = App::new();
        app.map_tool("search", |q: String| async move { q })
            .with_input_schema(|_| {
                crate::types::schema_2020::InputSchema::from_json_str(
                    r#"{
                        "type": "object",
                        "properties": { "query": { "type": "string" } },
                        "not": { "required": ["forbidden"] },
                        "additionalProperties": false
                    }"#,
                )
                .unwrap_or_default()
            })
            .with_arg_names(["q"]);

        app.validate_arg_names();
    }

    /// `additionalProperties: false` closes the schema; the map is exhaustive
    /// and the check applies as plainly as it does without the keyword.
    #[test]
    #[should_panic(expected = "declares the argument `q` but publishes an inputSchema without it")]
    #[cfg(not(feature = "legacy-spec"))]
    fn startup_still_checks_a_closed_schema() {
        let mut app = App::new();
        app.map_tool("search", |q: String| async move { q })
            .with_input_schema(|_| {
                crate::types::schema_2020::InputSchema::from_json_str(
                    r#"{"type":"object","properties":{"query":{"type":"string"}},"additionalProperties":false}"#,
                )
                .unwrap_or_default()
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
    #[should_panic(expected = "prompt `analyze` publishes the argument `topic` twice")]
    fn startup_rejects_a_prompt_with_duplicate_arg_names() {
        use crate::types::Role;

        let mut app = App::new();
        app.map_prompt("analyze", |topic: String, tone: String| async move {
            (format!("{topic}{tone}"), Role::User)
        })
        .with_args(["topic", "topic"]);

        app.validate_arg_names();
    }

    #[test]
    #[should_panic(expected = "tool `greet` declares the argument name `name` twice")]
    fn startup_rejects_a_tool_with_duplicate_arg_names() {
        let mut app = App::new();
        app.map_tool("greet", |first: String, last: String| async move {
            format!("{first}{last}")
        })
        .with_arg_names(["name", "name"]);

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

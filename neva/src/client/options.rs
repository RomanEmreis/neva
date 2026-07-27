//! MCP client options

use crate::PROTOCOL_VERSIONS;
use crate::client::notification_handler::NotificationsHandler;
use crate::transport::{StdIoClient, TransportProto, stdio::options::StdIoOptions};
use crate::types::SamplingCapability;
use crate::types::elicitation::ElicitationHandler;
use crate::types::sampling::SamplingHandler;
use crate::types::{ElicitationCapability, Implementation};
use crate::types::{Root, RootsCapability, Uri};
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "tasks")]
use crate::types::ClientTasksCapability;

#[cfg(feature = "http-client")]
use crate::transport::http::HttpClient;

const DEFAULT_REQUEST_TIMEOUT: u64 = 10; // 10 seconds

/// Default cap on MRTR re-issue rounds for a single request or a batched one.
#[cfg(not(feature = "legacy-spec"))]
const DEFAULT_MAX_MRTR_ROUNDS: usize = 8;

/// W3C Trace Context payload supplied by [`TraceContextProvider`] and
/// injected into the outbound request's `_meta`.
#[cfg(not(feature = "legacy-spec"))]
#[derive(Debug, Clone)]
pub struct TraceContext {
    /// `traceparent` carrier; always required when a context is returned.
    pub traceparent: String,
    /// Vendor-specific `tracestate`, when available.
    pub tracestate: Option<String>,
}

/// User-supplied callback that returns the current W3C Trace Context.
///
/// Invoked once per outbound request (before serialization). Return
/// `None` to omit trace headers from this request.
#[cfg(not(feature = "legacy-spec"))]
pub type TraceContextProvider = std::sync::Arc<dyn Fn() -> Option<TraceContext> + Send + Sync>;

/// Represents MCP client configuration options
pub struct McpOptions {
    /// Information of current client's implementation
    pub(crate) implementation: Implementation,

    /// Request timeout
    pub(super) timeout: Duration,

    /// Roots capability options
    pub(super) roots_capability: Option<RootsCapability>,

    /// Sampling capability options
    pub(super) sampling_capability: Option<SamplingCapability>,

    /// Elicitation capability options
    pub(super) elicitation_capability: Option<ElicitationCapability>,

    /// Client tasks capability options
    #[cfg(feature = "tasks")]
    pub(super) tasks_capability: Option<ClientTasksCapability>,

    /// Represents a handler function that runs when received a "sampling/createMessage" request
    pub(super) sampling_handler: Option<SamplingHandler>,

    /// Represents a handler function that runs when received a "elicitation/create" request
    pub(super) elicitation_handler: Option<ElicitationHandler>,

    /// Represents a hash map of notification handlers
    pub(super) notification_handler: Option<Arc<NotificationsHandler>>,

    /// An MCP version that a client supports
    protocol_ver: Option<&'static str>,

    /// Current transport protocol that the server uses
    proto: Option<TransportProto>,

    /// Represents a list of roots that the client supports
    roots: HashMap<Uri, Root>,

    /// Optional W3C Trace Context provider. Invoked before each outbound
    /// request; the returned tuple is injected into the request's `_meta`.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) trace_context_provider: Option<TraceContextProvider>,

    /// Cap on MRTR re-issue rounds before the client gives up on a request
    /// (guards against a server that never converges).
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) max_mrtr_rounds: usize,

    /// The dual-mode runtime switch: which protocol generation the
    /// connected peer speaks. Shared with the transport so per-request
    /// behavior (headers) follows the handshake outcome.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) peer_mode: crate::shared::PeerMode,

    /// Request-scoped logging level attached to every outbound request's
    /// `_meta["io.modelcontextprotocol/logLevel"]` (MCP 2026-07-28). Replaces
    /// the removed global `logging/setLevel`.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) log_level: Option<crate::types::notification::LoggingLevel>,
}

impl Debug for McpOptions {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut binding = f.debug_struct("McpOptions");
        let dbg = binding
            .field("implementation", &self.implementation)
            .field("timeout", &self.timeout)
            .field("elicitation_capability", &self.elicitation_capability);

        let dbg = dbg
            .field("roots_capability", &self.roots_capability)
            .field("sampling_capability", &self.sampling_capability);

        let dbg = dbg.field("protocol_ver", &self.protocol_ver);

        let dbg = dbg.field("roots", &self.roots);

        #[cfg(feature = "tasks")]
        dbg.field("tasks_capability", &self.tasks_capability);

        dbg.finish()
    }
}

impl Default for McpOptions {
    #[inline]
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(DEFAULT_REQUEST_TIMEOUT),
            implementation: Default::default(),
            roots: Default::default(),
            roots_capability: None,
            sampling_capability: None,
            elicitation_capability: None,
            #[cfg(feature = "tasks")]
            tasks_capability: None,
            proto: None,
            protocol_ver: None,
            sampling_handler: None,
            elicitation_handler: None,
            notification_handler: None,
            #[cfg(not(feature = "legacy-spec"))]
            trace_context_provider: None,
            #[cfg(not(feature = "legacy-spec"))]
            max_mrtr_rounds: DEFAULT_MAX_MRTR_ROUNDS,
            #[cfg(not(feature = "legacy-spec"))]
            peer_mode: Default::default(),
            #[cfg(not(feature = "legacy-spec"))]
            log_level: None,
        }
    }
}

impl McpOptions {
    /// Sets stdio as a transport protocol
    pub fn with_stdio<T>(mut self, command: &'static str, args: T) -> Self
    where
        T: IntoIterator<Item = &'static str>,
    {
        self.proto = Some(TransportProto::StdioClient(StdIoClient::new(
            StdIoOptions::new(command, args),
        )));
        self
    }

    /// Sets Streamable HTTP as a transport protocol
    #[cfg(feature = "http-client")]
    pub fn with_http<F: FnOnce(HttpClient) -> HttpClient>(mut self, config: F) -> Self {
        self.proto = Some(TransportProto::HttpClient(Box::new(config(
            HttpClient::default(),
        ))));
        self
    }

    /// Sets Streamable HTTP as a transport protocol with default configuration
    ///
    /// Default:
    /// * __IP__: 127.0.0.1
    /// * __PORT__: 3000
    /// * __ENDPOINT__: /mcp
    #[cfg(feature = "http-client")]
    pub fn with_default_http(self) -> Self {
        self.with_http(|http| http)
    }

    /// Specifies MCP client name
    pub fn with_name(mut self, name: &str) -> Self {
        self.implementation.name = name.into();
        self
    }

    /// Specifies MCP client version
    pub fn with_version(mut self, ver: &str) -> Self {
        self.implementation.version = ver.into();
        self
    }

    /// Specifies Model Context Protocol version
    ///
    /// Default: last available protocol version
    ///
    /// Under MCP 2026-07-28 the 2026-07-28 version itself is fixed -- the
    /// value set here only selects which **legacy** version the
    /// dual-mode fallback negotiates when a server rejects
    /// `server/discover` (default: the newest legacy version).
    pub fn with_mcp_version(mut self, ver: &'static str) -> Self {
        self.protocol_ver = Some(ver);
        self
    }

    /// Configures Roots capability
    #[deprecated(
        note = "Roots are deprecated in MCP 2026-07-28: the capability-driven `roots/list` request is gone and the ability is re-homed onto MRTR -- see `Context::list_roots`. Under MCP 2026-07-28 this configures what the client answers MRTR `roots/list` input requests with."
    )]
    pub fn with_roots<T>(mut self, config: T) -> Self
    where
        T: FnOnce(RootsCapability) -> RootsCapability,
    {
        self.roots_capability = Some(config(Default::default()));
        self
    }

    /// Configures Sampling capability
    #[deprecated(
        note = "Sampling is deprecated in MCP 2026-07-28: the capability-driven `sampling/createMessage` request is gone and the ability is re-homed onto MRTR -- see `Context::sample`."
    )]
    pub fn with_sampling<T>(mut self, config: T) -> Self
    where
        T: FnOnce(SamplingCapability) -> SamplingCapability,
    {
        self.sampling_capability = Some(config(Default::default()));
        self
    }

    /// Configures Elicitation capability
    pub fn with_elicitation<T>(mut self, config: T) -> Self
    where
        T: FnOnce(ElicitationCapability) -> ElicitationCapability,
    {
        self.elicitation_capability = Some(config(Default::default()));
        self
    }

    /// Configures tasks capability
    #[cfg(feature = "tasks")]
    pub fn with_tasks<T>(mut self, config: T) -> Self
    where
        T: FnOnce(ClientTasksCapability) -> ClientTasksCapability,
    {
        self.tasks_capability = Some(config(Default::default()));
        self
    }

    /// Specifies request timeout
    ///
    /// Default: 10 seconds
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets the maximum number of MRTR re-issue rounds the client drives for a
    /// single request (and per request across a batch) before giving up with an
    /// error. Guards against a server that keeps requesting input without ever
    /// converging.
    ///
    /// This counts *re-issues* only -- the initial send is always made on top of
    /// this budget. So `1` permits a normal one-question flow (initial send ->
    /// `input_required` -> one retry -> final), and `0` sends the request once and
    /// fails if it elicits at all.
    ///
    /// Default: 8.
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    ///
    /// let client = Client::new()
    ///     .with_options(|o| o.with_max_mrtr_rounds(16));
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub fn with_max_mrtr_rounds(mut self, rounds: usize) -> Self {
        self.max_mrtr_rounds = rounds;
        self
    }

    /// Sets the request-scoped logging level (MCP 2026-07-28).
    ///
    /// The level is attached to every outbound request's
    /// `_meta["io.modelcontextprotocol/logLevel"]`; the server delivers
    /// `notifications/message` at or above this severity while handling the
    /// request. This replaces the removed global `logging/setLevel` handshake.
    ///
    /// Deprecated on arrival: the 2026-07-28 draft marks the logging surface
    /// deprecated, and it is expected to be removed in a future revision.
    ///
    /// # Example
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::types::notification::LoggingLevel;
    ///
    /// # #[allow(deprecated)]
    /// let client = Client::new()
    ///     .with_options(|o| o.with_log_level(LoggingLevel::Warning));
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    #[deprecated(
        note = "Request-scoped logging is deprecated in MCP 2026-07-28 and may be removed in a future revision."
    )]
    pub fn with_log_level(mut self, level: crate::types::notification::LoggingLevel) -> Self {
        self.log_level = Some(level);
        self
    }

    /// Installs a W3C Trace Context provider. Called before each outbound
    /// request; the returned [`TraceContext`] is injected into `_meta`.
    #[cfg(not(feature = "legacy-spec"))]
    pub fn with_trace_context_provider<F>(mut self, f: F) -> Self
    where
        F: Fn() -> Option<TraceContext> + Send + Sync + 'static,
    {
        self.trace_context_provider = Some(std::sync::Arc::new(f));
        self
    }

    /// Returns a Model Context Protocol version that client supports
    ///
    /// Under MCP 2026-07-28 the 2026-07-28 version is pinned and the
    /// legacy override is read via
    /// [`legacy_protocol_ver`](Self::legacy_protocol_ver) instead.
    #[cfg(feature = "legacy-spec")]
    #[inline]
    pub(crate) fn protocol_ver(&self) -> &'static str {
        match self.protocol_ver {
            Some(ver) => ver,
            None => PROTOCOL_VERSIONS.last().unwrap(),
        }
    }

    /// Returns current transport protocol
    pub(crate) fn transport(&mut self) -> TransportProto {
        let transport = self.proto.take().unwrap_or_default();
        // Hand the dual-mode switch to the HTTP transport so request
        // headers follow the negotiated protocol generation. Only the
        // HTTP transport carries them, so this is gated on `http-client`
        // as well -- stdio-only 2026-07-28 clients (no `TransportProto::HttpClient`
        // variant at all) pass the transport through untouched.
        #[cfg(all(not(feature = "legacy-spec"), feature = "http-client"))]
        let transport = match transport {
            TransportProto::HttpClient(http) => {
                TransportProto::HttpClient(Box::new(http.with_peer_mode(self.peer_mode.clone())))
            }
            other => other,
        };
        transport
    }

    /// The newest legacy protocol version -- what the dual-mode fallback
    /// negotiates with a legacy peer. Honors a legacy
    /// [`with_mcp_version`](Self::with_mcp_version) override.
    #[cfg(not(feature = "legacy-spec"))]
    pub(crate) fn legacy_protocol_ver(&self) -> &'static str {
        match self.protocol_ver {
            Some(ver) if ver != crate::LATEST_PROTOCOL_VERSION => ver,
            _ => PROTOCOL_VERSIONS
                .iter()
                .rev()
                .find(|ver| **ver != crate::LATEST_PROTOCOL_VERSION)
                .copied()
                .unwrap_or("2025-11-25"),
        }
    }

    /// Adds a root
    pub fn add_root(&mut self, root: Root) -> &mut Root {
        self.roots.entry(root.uri.clone()).or_insert(root)
    }

    /// Adds multiple roots
    pub fn add_roots<T, I>(&mut self, roots: I) -> &mut Self
    where
        T: Into<Root>,
        I: IntoIterator<Item = T>,
    {
        let roots = roots.into_iter().map(|item| {
            let root: Root = item.into();
            (root.uri.clone(), root)
        });
        self.roots.extend(roots);
        self
    }

    /// Returns a list of defined Roots
    pub fn roots(&self) -> Vec<Root> {
        self.roots.values().cloned().collect()
    }

    /// Registers a handler for sampling requests
    pub(crate) fn add_sampling_handler(&mut self, handler: SamplingHandler) {
        self.sampling_handler = Some(handler);
    }

    /// Registers a handler for elicitation requests
    pub(crate) fn add_elicitation_handler(&mut self, handler: ElicitationHandler) {
        self.elicitation_handler = Some(handler);
    }

    /// Returns [`RootsCapability`] if configured.
    /// If not configured but at least one [`Root`] exists, returns [`Default`].
    /// Otherwise, returns `None`.
    pub(crate) fn roots_capability(&self) -> Option<RootsCapability> {
        self.roots_capability
            .clone()
            .or_else(|| (!self.roots.is_empty()).then(Default::default))
    }

    /// Returns [`SamplingCapability`] if configured.
    /// If not configured but a sampling handler exists, it returns [`Default`].
    /// Otherwise, returns `None`.
    pub(crate) fn sampling_capability(&self) -> Option<SamplingCapability> {
        self.sampling_capability
            .clone()
            .or_else(|| self.sampling_handler.is_some().then(Default::default))
    }

    /// Returns [`ElicitationCapability`] if configured.
    /// If not configured but an elicitation handler exists, it returns [`Default`].
    /// Otherwise, returns `None`.
    pub(crate) fn elicitation_capability(&self) -> Option<ElicitationCapability> {
        self.elicitation_capability
            .clone()
            .or_else(|| self.elicitation_handler.is_some().then(Default::default))
    }

    /// Returns [`ClientTasksCapability`] if configured.
    ///
    /// Otherwise, returns `None`.
    #[cfg(feature = "tasks")]
    pub(crate) fn tasks_capability(&self) -> Option<ClientTasksCapability> {
        self.tasks_capability.clone()
    }
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;

    /// An explicit `with_roots(..)` must survive an empty roots list: the MRTR
    /// flag is derived from this, and an empty `ListRootsResult` is a perfectly
    /// valid answer -- a client that opted in must still be askable.
    #[test]
    fn an_explicit_roots_capability_survives_an_empty_list() {
        #[allow(deprecated)]
        let opts = McpOptions::default().with_roots(|roots| roots);
        assert!(opts.roots().is_empty(), "no roots were added");
        assert!(
            opts.roots_capability().is_some(),
            "an explicit opt-in must not be dropped just because the list is empty"
        );

        // ...and with neither an opt-in nor any roots there is nothing to declare.
        let bare = McpOptions::default();
        assert!(bare.roots_capability().is_none());
    }

    /// Implicit sampling/elicitation capabilities follow the *installed
    /// handler*: advertising one a client cannot serve makes a legacy
    /// server send requests answered with `MethodNotFound`, while hiding
    /// one that is installed makes it skip the feature entirely.
    #[test]
    fn implicit_capabilities_follow_the_installed_handlers() {
        let mut opts = McpOptions::default();
        assert!(
            opts.sampling_capability().is_none(),
            "no sampling handler -- nothing to advertise"
        );
        assert!(
            opts.elicitation_capability().is_none(),
            "no elicitation handler -- nothing to advertise"
        );

        opts.add_sampling_handler(Arc::new(|_params| {
            Box::pin(async move { crate::types::sampling::CreateMessageResult::assistant() })
        }));
        opts.add_elicitation_handler(Arc::new(|_params| {
            Box::pin(async move { crate::types::elicitation::ElicitResult::decline() })
        }));

        assert!(
            opts.sampling_capability().is_some(),
            "an installed sampling handler must be advertised"
        );
        assert!(
            opts.elicitation_capability().is_some(),
            "an installed elicitation handler must be advertised"
        );
    }

    #[test]
    #[cfg(not(feature = "legacy-spec"))]
    fn trace_context_provider_can_be_installed() {
        let opts = McpOptions::default().with_trace_context_provider(|| {
            Some(TraceContext {
                traceparent: "tp".into(),
                tracestate: Some("ts".into()),
            })
        });
        let tc = (opts.trace_context_provider.as_ref().unwrap())().unwrap();
        assert_eq!(tc.traceparent, "tp");
        assert_eq!(tc.tracestate.as_deref(), Some("ts"));
    }
}

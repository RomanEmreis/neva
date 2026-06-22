//! MCP client options

use crate::PROTOCOL_VERSIONS;
use crate::client::notification_handler::NotificationsHandler;
use crate::transport::{StdIoClient, TransportProto, stdio::options::StdIoOptions};
#[cfg(not(feature = "proto-2026-07-28-rc"))]
use crate::types::SamplingCapability;
use crate::types::elicitation::ElicitationHandler;
#[cfg(not(feature = "proto-2026-07-28-rc"))]
use crate::types::sampling::SamplingHandler;
use crate::types::{ElicitationCapability, Implementation};
#[cfg(not(feature = "proto-2026-07-28-rc"))]
use crate::types::{Root, RootsCapability, Uri};
#[cfg(not(feature = "proto-2026-07-28-rc"))]
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
#[cfg(feature = "proto-2026-07-28-rc")]
const DEFAULT_MAX_MRTR_ROUNDS: usize = 8;

/// W3C Trace Context payload supplied by [`TraceContextProvider`] and
/// injected into the outbound request's `_meta`.
#[cfg(feature = "proto-2026-07-28-rc")]
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
#[cfg(feature = "proto-2026-07-28-rc")]
pub type TraceContextProvider = std::sync::Arc<dyn Fn() -> Option<TraceContext> + Send + Sync>;

/// Represents MCP client configuration options
pub struct McpOptions {
    /// Information of current client's implementation
    pub(crate) implementation: Implementation,

    /// Request timeout
    pub(super) timeout: Duration,

    /// Roots capability options
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
    pub(super) roots_capability: Option<RootsCapability>,

    /// Sampling capability options
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
    pub(super) sampling_capability: Option<SamplingCapability>,

    /// Elicitation capability options
    pub(super) elicitation_capability: Option<ElicitationCapability>,

    /// Client tasks capability options
    #[cfg(feature = "tasks")]
    pub(super) tasks_capability: Option<ClientTasksCapability>,

    /// Represents a handler function that runs when received a "sampling/createMessage" request
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
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
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
    roots: HashMap<Uri, Root>,

    /// Optional W3C Trace Context provider. Invoked before each outbound
    /// request; the returned tuple is injected into the request's `_meta`.
    #[cfg(feature = "proto-2026-07-28-rc")]
    pub(crate) trace_context_provider: Option<TraceContextProvider>,

    /// Cap on MRTR re-issue rounds before the client gives up on a request
    /// (guards against a server that never converges).
    #[cfg(feature = "proto-2026-07-28-rc")]
    pub(crate) max_mrtr_rounds: usize,
}

impl Debug for McpOptions {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut binding = f.debug_struct("McpOptions");
        let dbg = binding
            .field("implementation", &self.implementation)
            .field("timeout", &self.timeout)
            .field("elicitation_capability", &self.elicitation_capability);

        #[cfg(not(feature = "proto-2026-07-28-rc"))]
        let dbg = dbg
            .field("roots_capability", &self.roots_capability)
            .field("sampling_capability", &self.sampling_capability);

        let dbg = dbg.field("protocol_ver", &self.protocol_ver);

        #[cfg(not(feature = "proto-2026-07-28-rc"))]
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
            #[cfg(not(feature = "proto-2026-07-28-rc"))]
            roots: Default::default(),
            #[cfg(not(feature = "proto-2026-07-28-rc"))]
            roots_capability: None,
            #[cfg(not(feature = "proto-2026-07-28-rc"))]
            sampling_capability: None,
            elicitation_capability: None,
            #[cfg(feature = "tasks")]
            tasks_capability: None,
            proto: None,
            protocol_ver: None,
            #[cfg(not(feature = "proto-2026-07-28-rc"))]
            sampling_handler: None,
            elicitation_handler: None,
            notification_handler: None,
            #[cfg(feature = "proto-2026-07-28-rc")]
            trace_context_provider: None,
            #[cfg(feature = "proto-2026-07-28-rc")]
            max_mrtr_rounds: DEFAULT_MAX_MRTR_ROUNDS,
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
        self.proto = Some(TransportProto::HttpClient(config(HttpClient::default())));
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
    /// Not available under `proto-2026-07-28-rc`: that flag compiles the client
    /// as a pure 2026-07-28 RC peer (sampling/roots removed, stateless transport,
    /// MRTR), so negotiating an older version would advertise a protocol the
    /// build cannot actually speak. The RC version is fixed and sent on every
    /// request. When the RC graduates and the flags invert, version selection
    /// returns under the legacy flag.
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
    pub fn with_mcp_version(mut self, ver: &'static str) -> Self {
        self.protocol_ver = Some(ver);
        self
    }

    /// Configures Roots capability
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
    #[deprecated(
        note = "Roots are removed in MCP 2026-07-28; this method will be removed when the legacy flag is dropped."
    )]
    pub fn with_roots<T>(mut self, config: T) -> Self
    where
        T: FnOnce(RootsCapability) -> RootsCapability,
    {
        self.roots_capability = Some(config(Default::default()));
        self
    }

    /// Configures Sampling capability
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
    #[deprecated(
        note = "Sampling is removed in MCP 2026-07-28; this method will be removed when the legacy flag is dropped."
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
    /// This counts *re-issues* only — the initial send is always made on top of
    /// this budget. So `1` permits a normal one-question flow (initial send →
    /// `input_required` → one retry → final), and `0` sends the request once and
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
    #[cfg(feature = "proto-2026-07-28-rc")]
    pub fn with_max_mrtr_rounds(mut self, rounds: usize) -> Self {
        self.max_mrtr_rounds = rounds;
        self
    }

    /// Installs a W3C Trace Context provider. Called before each outbound
    /// request; the returned [`TraceContext`] is injected into `_meta`.
    #[cfg(feature = "proto-2026-07-28-rc")]
    pub fn with_trace_context_provider<F>(mut self, f: F) -> Self
    where
        F: Fn() -> Option<TraceContext> + Send + Sync + 'static,
    {
        self.trace_context_provider = Some(std::sync::Arc::new(f));
        self
    }

    /// Returns a Model Context Protocol version that client supports
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

    /// Adds a root
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
    pub fn add_root(&mut self, root: Root) -> &mut Root {
        self.roots.entry(root.uri.clone()).or_insert(root)
    }

    /// Adds multiple roots
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
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
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
    pub fn roots(&self) -> Vec<Root> {
        self.roots.values().cloned().collect()
    }

    /// Registers a handler for sampling requests
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
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
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
    pub(crate) fn roots_capability(&self) -> Option<RootsCapability> {
        self.roots_capability
            .clone()
            .or_else(|| (!self.roots.is_empty()).then(Default::default))
    }

    /// Returns [`SamplingCapability`] if configured.
    /// If not configured but a sampling handler exists, it returns [`Default`].
    /// Otherwise, returns `None`.
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
    pub(crate) fn sampling_capability(&self) -> Option<SamplingCapability> {
        self.sampling_capability
            .clone()
            .or_else(|| self.sampling_handler.is_none().then(Default::default))
    }

    /// Returns [`ElicitationCapability`] if configured.
    /// If not configured but an elicitation handler exists, it returns [`Default`].
    /// Otherwise, returns `None`.
    #[cfg(not(feature = "proto-2026-07-28-rc"))]
    pub(crate) fn elicitation_capability(&self) -> Option<ElicitationCapability> {
        self.elicitation_capability
            .clone()
            .or_else(|| self.elicitation_handler.is_none().then(Default::default))
    }

    /// Returns [`ClientTasksCapability`] if configured.
    ///
    /// Otherwise, returns `None`.
    #[cfg(all(feature = "tasks", not(feature = "proto-2026-07-28-rc")))]
    pub(crate) fn tasks_capability(&self) -> Option<ClientTasksCapability> {
        self.tasks_capability.clone()
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "proto-2026-07-28-rc")]
    use super::*;

    #[test]
    #[cfg(feature = "proto-2026-07-28-rc")]
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

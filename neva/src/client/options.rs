//! MCP client options

use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;
use crate::PROTOCOL_VERSIONS;
use crate::transport::{StdIoClient, stdio::options::StdIoOptions, TransportProto};
use crate::client::notification_handler::NotificationsHandler;
use crate::types::sampling::SamplingHandler;
use crate::types::elicitation::ElicitationHandler;
use crate::types::{
    Root, 
    Implementation, 
    Uri, 
    RootsCapability, 
    SamplingCapability, 
    ElicitationCapability,
    ClientTasksCapability
};

#[cfg(feature = "http-client")]
use crate::transport::http::HttpClient;

const DEFAULT_REQUEST_TIMEOUT: u64 = 10; // 10 seconds

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
}

impl Debug for McpOptions {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpOptions")
            .field("implementation", &self.implementation)
            .field("timeout", &self.timeout)
            .field("roots_capability", &self.roots_capability)
            .field("elicitation_capability", &self.elicitation_capability)
            .field("sampling_capability", &self.sampling_capability)
            .field("protocol_ver", &self.protocol_ver)
            .field("roots", &self.roots)
            .finish()
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
            tasks_capability: None,
            proto: None,
            protocol_ver: None,
            sampling_handler: None,
            elicitation_handler: None,
            notification_handler: None
        }
    }
}

impl McpOptions {
    /// Sets stdio as a transport protocol
    pub fn with_stdio<T>(mut self, command: &'static str, args: T) -> Self
    where
        T: IntoIterator<Item=&'static str>
    {
        self.proto = Some(TransportProto::StdioClient(StdIoClient::new(StdIoOptions::new(command, args))));
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
    pub fn with_mcp_version(mut self, ver: &'static str) -> Self {
        self.protocol_ver = Some(ver);
        self
    }
    
    /// Configures Roots capability
    pub fn with_roots<T>(mut self, config: T) -> Self
    where 
        T: FnOnce(RootsCapability) -> RootsCapability
    {
        self.roots_capability = Some(config(Default::default()));
        self
    }

    /// Configures Sampling capability
    pub fn with_sampling<T>(mut self, config: T) -> Self
    where
        T: FnOnce(SamplingCapability) -> SamplingCapability
    {
        self.sampling_capability = Some(config(Default::default()));
        self
    }

    /// Configures Elicitation capability
    pub fn with_elicitation<T>(mut self, config: T) -> Self
    where
        T: FnOnce(ElicitationCapability) -> ElicitationCapability
    {
        self.elicitation_capability = Some(config(Default::default()));
        self
    }

    /// Configures tasks capability
    pub fn with_tasks<T>(mut self, config: T) -> Self
    where
        T: FnOnce(ClientTasksCapability) -> ClientTasksCapability
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

    /// Returns a Model Context Protocol version that client supports
    #[inline]
    pub(crate) fn protocol_ver(&self) -> &'static str {
        match self.protocol_ver {
            Some(ver) => ver,
            None => PROTOCOL_VERSIONS.last().unwrap()
        }
    }

    /// Returns current transport protocol
    pub(crate) fn transport(&mut self) -> TransportProto {
        let transport = self.proto.take();
        transport.unwrap_or_default()
    }
    
    /// Adds a root
    pub fn add_root(&mut self, root: Root) -> &mut Root {
        self.roots
            .entry(root.uri.clone())
            .or_insert(root)
    }

    /// Adds multiple roots
    pub fn add_roots<T, I>(&mut self, roots: I) -> &mut Self
    where
        T: Into<Root>,
        I: IntoIterator<Item = T>
    {
        let roots = roots
            .into_iter()
            .map(|item| {
                let root: Root = item.into();
                (root.uri.clone(), root)
            });
        self.roots.extend(roots);
        self    
    }
    
    /// Returns a list of defined Roots
    pub fn roots(&self) -> Vec<Root> {
        self.roots
            .values()
            .cloned()
            .collect()
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
            .or_else(|| self.sampling_handler.is_none().then(Default::default))
    }

    /// Returns [`ElicitationCapability`] if configured.
    /// If not configured but an elicitation handler exists, it returns [`Default`].
    /// Otherwise, returns `None`.
    pub(crate) fn elicitation_capability(&self) -> Option<ElicitationCapability> {
        self.elicitation_capability
            .clone()
            .or_else(|| self.elicitation_handler.is_none().then(Default::default))
    }

    /// Returns [`ClientTasksCapability`] if configured.
    /// 
    /// Otherwise, returns `None`.
    pub(crate) fn tasks_capability(&self) -> Option<ClientTasksCapability> {
        self.tasks_capability.clone()
    }
}




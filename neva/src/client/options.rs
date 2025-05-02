//! MCP client options

use std::collections::HashMap;
use std::time::Duration;
use crate::PROTOCOL_VERSIONS;
use crate::transport::{StdIoClient, stdio::options::StdIoOptions, TransportProto};
use crate::types::capabilities::{RootsCapability, SamplingCapability};
use crate::types::{Root, Implementation, Uri};
use crate::types::sampling::SamplingHandler;

const DEFAULT_REQUEST_TIMEOUT: u64 = 10;

/// Represents MCP client configuration options
pub struct McpOptions {
    /// Information of current server's implementation
    pub(crate) implementation: Implementation,
    
    /// Request timeout
    pub(super) timeout: Duration,
    
    /// Roots capability options
    pub(super) roots_capability: Option<RootsCapability>,
    
    /// Sampling capability options
    pub(super) sampling_capability: Option<SamplingCapability>,

    /// Represents a handler function that runs when received a "sampling/createMessage" request
    pub(super) sampling_handler: Option<SamplingHandler>,
    
    /// An MCP version that server supports
    protocol_ver: Option<&'static str>,

    /// Current transport protocol that server uses
    proto: Option<TransportProto>,
    
    /// Represents a list of roots that the client supports
    roots: HashMap<Uri, Root>,
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
            proto: None,
            protocol_ver: None,
            sampling_handler: None
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

    /// Returns [`RootsCapability`] if configured.
    /// If not configured but at least one [`Root`] exists, returns [`Default`].
    /// Otherwise, returns `None`.
    pub(crate) fn roots_capability(&self) -> Option<RootsCapability> {
        self.roots_capability
            .clone()
            .or_else(|| (!self.roots.is_empty()).then(Default::default))
    }

    /// Returns [`SamplingCapability`] if configured.
    /// If not configured but a sampling handler exists, returns [`Default`].
    /// Otherwise, returns `None`.
    pub(crate) fn sampling_capability(&self) -> Option<SamplingCapability> {
        self.sampling_capability
            .clone()
            .or_else(|| self.sampling_handler.is_none().then(Default::default))
    }
}




//! MCP client options

use std::collections::HashMap;
use std::time::Duration;
use crate::PROTOCOL_VERSIONS;
use crate::transport::{StdIo, stdio::options::StdIoOptions, TransportProto};
use crate::types::capabilities::{RootsCapability, SamplingCapability};
use crate::types::{Root, Implementation, Uri};

/// Represents MCP client configuration options
pub struct McpOptions {
    /// Information of current server's implementation
    pub(crate) implementation: Implementation,
    
    /// Roots capability options
    pub(crate) roots_capability: Option<RootsCapability>,
    
    /// Sampling capability options
    pub(crate) sampling_capability: Option<SamplingCapability>,

    /// Request timeout
    pub(super) timeout: Duration,
    
    /// An MCP version that server supports
    protocol_ver: Option<&'static str>,

    /// Current transport protocol that server uses
    proto: Option<TransportProto>,
    
    /// Represents a list of roots that the client supports
    roots: HashMap<Uri, Root>
}

impl Default for McpOptions {
    #[inline]
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            implementation: Default::default(),
            roots: Default::default(),
            roots_capability: None,
            sampling_capability: None,
            proto: None,
            protocol_ver: None,
        }
    }
}

impl McpOptions {
    /// Sets stdio as a transport protocol
    pub fn with_stdio<T>(mut self, command: &'static str, args: T) -> Self
    where
        T: IntoIterator<Item=&'static str>
    {
        self.proto = Some(TransportProto::Stdio(StdIo::client(StdIoOptions::new(command, args))));
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
    
    /// Returns a list of defined Roots
    pub fn roots(&self) -> Vec<Root> {
        self.roots
            .values()
            .cloned()
            .collect()
    }
}




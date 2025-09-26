//! MCP client TLS configuration

use std::path::PathBuf;
use reqwest::{Certificate, Identity};
use crate::error::Error;

/// Represents TLS configuration for MCP client
pub struct TlsConfig {
    ca_path: Option<PathBuf>,
    cert_path: Option<PathBuf>,
}

/// Represents TLS certificates secrets
#[derive(Clone)]
pub(crate) struct ClientTlsConfig {
    pub(crate) ca: Option<Certificate>,
    pub(crate) identity: Option<Identity>,
}

impl Default for TlsConfig {
    #[inline]
    fn default() -> Self {
        Self {
            ca_path: None,
            cert_path: None,
        }
    }
}

impl TlsConfig {
    /// Sets the path to certificate file
    pub fn with_cert(mut self, cert: impl Into<PathBuf>) -> Self {
        self.cert_path = Some(cert.into());
        self
    }
    
    /// Sets the path to CA (Client Authority) file
    pub fn with_ca(mut self, path: impl Into<PathBuf>) -> Self {
        self.ca_path = Some(path.into());
        self
    }
    
    /// Creates MCP client TLS config
    pub(crate) fn build(self) -> Result<ClientTlsConfig, Error> {
        let ca = if let Some(ca_path) = self.ca_path {
            let ca = std::fs::read(ca_path)
                .map_err(Error::from)
                .and_then(|b| Certificate::from_pem(&b).map_err(Into::into))?;
            Some(ca)
        } else { 
            None
        };

        let identity = if let Some(cert_path) = self.cert_path {
            let identity = std::fs::read(cert_path)
                .map_err(Error::from)
                .and_then(|b| Identity::from_pem(&b).map_err(Into::into))?;
            Some(identity)
        } else {
            None
        };

        Ok(ClientTlsConfig { ca, identity })
    }
}
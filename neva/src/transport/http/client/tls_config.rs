//! MCP client TLS configuration

use std::path::PathBuf;
use reqwest::{Certificate, Identity};
use crate::error::Error;

/// Represents TLS configuration for an MCP client
#[derive(Debug)]
pub struct TlsConfig {
    ca_path: Option<PathBuf>,
    cert_path: Option<PathBuf>,
    certs_verification: bool,
}

/// Represents TLS certificates secrets
#[derive(Clone)]
pub(crate) struct ClientTlsConfig {
    pub(crate) ca: Option<Certificate>,
    pub(crate) identity: Option<Identity>,
    pub(crate) certs_verification: bool,
}

impl Default for TlsConfig {
    #[inline]
    fn default() -> Self {
        Self {
            ca_path: None,
            cert_path: None,
            certs_verification: true,
        }
    }
}

impl TlsConfig {
    /// Sets the path to a certificate file
    pub fn with_cert(mut self, cert: impl Into<PathBuf>) -> Self {
        self.cert_path = Some(cert.into());
        self
    }
    
    /// Sets the path to the CA (Client Authority) file
    pub fn with_ca(mut self, path: impl Into<PathBuf>) -> Self {
        self.ca_path = Some(path.into());
        self
    }
    
    /// Controls the use of certificate validation.
    /// Setting this to `false` disables TLS certificate validation.
    /// 
    /// Default: `true`.
    ///
    /// # Warning
    ///
    /// You should think very carefully before using this method. If
    /// invalid certificates are trusted, *any* certificate for *any* site
    /// will be trusted for use. This includes expired certificates. This
    /// introduces significant vulnerabilities and should only be used
    /// as a last resort.
    pub fn with_certs_verification(mut self, certs_verification: bool) -> Self {
        self.certs_verification = certs_verification;
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

        Ok(ClientTlsConfig { ca, identity, certs_verification: self.certs_verification })
    }
}
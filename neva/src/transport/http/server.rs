//! HTTP server module — under the `http-server-volga` default feature
//! this re-exports the Volga adapter; under bare `http-server` alone
//! it is essentially empty.

#[cfg(feature = "http-server")]
pub(crate) use crate::transport::http::core::types::DefaultClaims;

#[cfg(feature = "http-server-volga")]
pub(crate) mod volga;

#[cfg(feature = "http-server-volga")]
pub(crate) use volga::auth_config::AuthConfig;

#[cfg(feature = "http-server-volga")]
pub(crate) use volga::VolgaEngine;

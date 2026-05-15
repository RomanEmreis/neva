//! Volga-based default implementation of
//! [`HttpEngine`](crate::transport::http::core::engine::HttpEngine).
//!
//! This is the engine bound by default when the `http-server-volga`
//! feature is enabled. It implements `HttpEngine` by binding a
//! `volga::App`, registering three routes on the MCP endpoint, and
//! delegating all protocol work to the engine-agnostic helpers in
//! [`super::super::core::handlers`].

pub(crate) mod auth_config;
pub(crate) mod engine;
pub(crate) mod responder;
pub(crate) mod routes;

pub(crate) use engine::VolgaEngine;

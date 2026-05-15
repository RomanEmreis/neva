//! Engine-agnostic Streamable HTTP transport primitives.
//!
//! This module owns the protocol-level logic of the MCP Streamable HTTP
//! transport — JSON-RPC framing, SSE replay/dispatch, and the request/response
//! types that flow through engine adapters. Engines (Volga, Axum, custom)
//! implement [`HttpEngine`] and call the free helpers in [`handlers`].

pub mod context;
pub mod engine;
pub mod types;

pub(crate) mod cleanup;
pub(crate) mod dispatch;

pub mod handlers;

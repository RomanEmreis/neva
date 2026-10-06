//! The svir bridge: MCP tools handed to a model.
//!
//! [svir](https://docs.rs/svir) is the model side of the same family: tool
//! descriptors, calls and results for function calling, and the
//! [`Toolbox`](::svir::Toolbox) trait a request takes its tools from. It has no
//! MCP of its own; this module implements `Toolbox` over MCP.
//!
//! - [`RemoteTools`]: the tools of a connected [`Client`](crate::Client).
//!
//! A model takes less than MCP carries, and nothing is lost silently:
//!
//! - **Names.** A function name is `[a-zA-Z0-9_-]{1,64}`, where MCP also allows
//!   `.` and up to 128 characters. A tool whose name a model API cannot carry
//!   fails the listing rather than being renamed behind the caller's back;
//!   the caller can rename it.
//! - **Results.** A tool result is a string. Text blocks are joined with a
//!   blank line; structured content is sent as JSON text when there is no
//!   text; an embedded text resource is its text, and a resource link its name
//!   and URI. Images, audio and binary resources answer an error naming what
//!   could not be passed on.
//! - **Visibility.** With the `apps` feature, a tool MCP Apps hides from the
//!   model is never offered.
//!
//! The feature brings svir in without its default features: the types and the
//! `Toolbox` trait, no HTTP client. The model is called through svir directly.

mod convert;
mod remote;

pub use remote::RemoteTools;

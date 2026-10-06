//! The svir bridge: MCP tools handed to a model.
//!
//! [svir](https://docs.rs/svir) is the model side of the same family: tool
//! descriptors, calls and results for function calling, and the
//! [`Toolbox`](::svir::Toolbox) trait a request takes its tools from. It has no
//! MCP of its own; this module implements `Toolbox` over MCP.
//!
//! - `RemoteTools` (feature `client`): the tools of a connected `Client`.
//! - `LocalTools` (feature `server`): this server's own tools, called in the
//!   same process through `App::into_toolbox`.
//! - [`prompt_messages`] and [`resource_parts`]: a prompt as the messages it
//!   opens a conversation with, and a resource as the parts attached to one.
//!   They are not tools: in MCP the user picks a prompt and the application
//!   picks a resource, so neither is the model's to call.
//! - `sampling_request` and `sampling_result` (feature `client`): a server's
//!   `sampling/createMessage` as a request to a model, and the model's answer
//!   as the sample, for a `Client::map_sampling` handler to call the model
//!   in between. A `svir::Error` converts into neva's, so `?` passes a
//!   model's failure on to the server.
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
#[cfg(feature = "server")]
mod local;
mod offer;
#[cfg(feature = "client")]
mod remote;
#[cfg(feature = "client")]
mod sampling;

pub use convert::{prompt_messages, resource_parts};
#[cfg(feature = "server")]
pub use local::LocalTools;
#[cfg(feature = "client")]
pub use remote::RemoteTools;
#[cfg(feature = "client")]
pub use sampling::{sampling_request, sampling_result};

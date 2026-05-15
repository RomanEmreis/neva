//! Neutral request/response types and the `Claims` contract.
//!
//! These types are the common currency between neva's protocol helpers
//! and any HTTP engine. Built on the de-facto-standard `http` crate so
//! conversion to/from any framework (Volga, Axum, Hyper, Actix) is
//! either zero-cost or a single `.into_parts()` call.

use bytes::Bytes;
use http::HeaderMap;

/// Engine-neutral inbound HTTP request.
///
/// The body is fully buffered to [`Bytes`] before this type is constructed —
/// MCP messages are bounded JSON-RPC frames and the protocol handlers always
/// buffer the whole body before parsing.
///
/// # Example
///
/// ```rust,ignore
/// let req: HttpRequest = http::Request::builder()
///     .method("POST")
///     .uri("/mcp")
///     .body(bytes::Bytes::from_static(br#"{"jsonrpc":"2.0","method":"ping","id":1}"#))
///     .unwrap();
/// ```
pub type HttpRequest = http::Request<Bytes>;

/// Engine-neutral outbound HTTP response (non-SSE).
///
/// # Example
///
/// ```rust,ignore
/// let resp: HttpResponse = http::Response::builder()
///     .status(202)
///     .body(bytes::Bytes::new())
///     .unwrap();
/// ```
pub type HttpResponse = http::Response<Bytes>;

/// Outcome of the GET handler: either an SSE stream or a non-SSE status reply.
///
/// `Stream` is the happy path — 200 OK + the event stream.
/// `Status` is returned when the request couldn't establish a session
/// (typically 400 with no `Mcp-Session-Id` header).
#[derive(Debug)]
pub enum SseResponse<S> {
    /// 200 OK with an SSE event stream.
    Stream {
        /// Response headers (typically just `Mcp-Session-Id`).
        headers: HeaderMap,
        /// Stream of engine-native SSE event values.
        stream: S,
    },
    /// Non-streaming status response (e.g. 400 when no session id is provided).
    Status(HttpResponse),
}

/// Bridge that turns neva's per-event protocol data into the engine's
/// native SSE event type, so the framework boundary does zero conversion.
///
/// Two constructors cover every event the MCP transport emits today:
/// `tracked` (with an `id:` field, counts toward Last-Event-ID replay) and
/// `ephemeral` (without an `id:` field, e.g. tracing log notifications).
///
/// # Example
///
/// ```rust,ignore
/// struct PrintResponder;
/// impl SseResponder for PrintResponder {
///     type Event = String;
///     fn tracked(&self, seq: u64, msg: &neva::types::Message) -> String {
///         format!("id:{seq} data:{}", serde_json::to_string(msg).unwrap())
///     }
///     fn ephemeral(&self, msg: &neva::types::Message) -> String {
///         format!("data:{}", serde_json::to_string(msg).unwrap())
///     }
/// }
/// ```
pub trait SseResponder: Send + Sync + 'static {
    /// Engine-native SSE event type (e.g. `volga::http::sse::Message`).
    type Event: Send + 'static;
    /// Build an event WITH an `id:` field (advances client's Last-Event-ID).
    fn tracked(&self, seq: u64, msg: &crate::types::Message) -> Self::Event;
    /// Build an event WITHOUT an `id:` field (ephemeral log/notification).
    fn ephemeral(&self, msg: &crate::types::Message) -> Self::Event;
}

/// Typed claims contract used by neva's per-tool authorization checks.
///
/// Identical shape to `volga::auth::AuthClaims`. When the
/// `http-server-volga` feature is active, [`crate::auth::Claims`] is a
/// re-export of `volga::auth::AuthClaims`; this local definition is used
/// only in the engine-agnostic build.
///
/// # Example
///
/// ```rust,ignore
/// struct MyClaims { sub: String, role: String }
///
/// impl neva::transport::http::core::types::Claims for MyClaims {
///     fn role(&self) -> Option<&str> { Some(&self.role) }
/// }
/// ```
#[cfg(not(feature = "http-server-volga"))]
pub trait Claims: Send + Sync + 'static {
    /// Single role for this subject, if any.
    fn role(&self) -> Option<&str> {
        None
    }
    /// Multiple roles for this subject, if any.
    fn roles(&self) -> Option<&[String]> {
        None
    }
    /// Permission set for this subject, if any.
    fn permissions(&self) -> Option<&[String]> {
        None
    }
}

/// Default claims used when no custom claims type is configured.
///
/// Identical fields and serde shape to the Volga-flavored `DefaultClaims`.
/// The local copy is used only in the non-Volga build.
#[cfg(not(feature = "http-server-volga"))]
#[derive(Default, Clone, Debug, serde::Deserialize)]
pub struct DefaultClaims {
    /// JWT `sub` claim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
    /// Subject role.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Subject roles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<String>>,
    /// Subject permissions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Vec<String>>,
}

#[cfg(not(feature = "http-server-volga"))]
impl Claims for DefaultClaims {
    fn role(&self) -> Option<&str> {
        self.role.as_deref()
    }
    fn roles(&self) -> Option<&[String]> {
        self.roles.as_deref()
    }
    fn permissions(&self) -> Option<&[String]> {
        self.permissions.as_deref()
    }
}

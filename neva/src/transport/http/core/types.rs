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

/// Typed claims contract used by neva's per-tool authorization checks.
///
/// This is neva's engine-neutral trait. Engine adapters that want their
/// own claims type (axum, hyper, ...) implement this trait so that
/// `with_roles` / `with_permissions` on tools, prompts, and resources
/// continue to gate access regardless of which HTTP stack delivered the
/// request.
///
/// Under the default Volga adapter, `volga::auth::AuthClaims` is also
/// re-exported as [`crate::auth::Claims`], and the Volga-flavored
/// `DefaultClaims` implements this trait too — so the same validator
/// runs for every engine.
///
/// # Example
///
/// ```rust,ignore
/// #[derive(Debug)]
/// struct MyClaims { sub: String, role: String }
///
/// impl neva::auth::Claims for MyClaims {
///     fn role(&self) -> Option<&str> { Some(&self.role) }
/// }
/// ```
///
/// `Debug` is required so that `Request` (which derives `Debug`) can
/// hold an `Arc<dyn Claims>`.
pub trait Claims: std::fmt::Debug + Send + Sync + 'static {
    /// Authenticated subject (principal) for this request, if any.
    ///
    /// Used to bind MRTR `requestState` to the principal that produced it
    /// under `proto-2026-07-28-rc`. Defaults to `None`.
    fn subject(&self) -> Option<&str> {
        None
    }
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

/// Engine-agnostic pre-built [`Claims`] type matching the JWT standard
/// claim names. Available for every HTTP engine — under the Volga
/// adapter it also implements `volga::auth::AuthClaims` so it can be
/// fed straight into Volga's bearer-auth pipeline.
#[derive(Default, Clone, Debug, serde::Deserialize)]
pub struct DefaultClaims {
    /// JWT `sub` claim — subject.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
    /// JWT `iss` claim — issuer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iss: Option<String>,
    /// JWT `aud` claim — audience.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aud: Option<String>,
    /// JWT `exp` claim — expiration time (seconds since epoch).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp: Option<i64>,
    /// JWT `nbf` claim — not-before time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nbf: Option<i64>,
    /// JWT `iat` claim — issued-at time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iat: Option<i64>,
    /// JWT `jti` claim — token id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jti: Option<String>,
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

impl Claims for DefaultClaims {
    fn subject(&self) -> Option<&str> {
        self.sub.as_deref()
    }
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

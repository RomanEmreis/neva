//! Neutral request/response types and the `Claims` contract.
//!
//! These types are the common currency between neva's protocol helpers
//! and any HTTP engine. Built on the de-facto-standard `http` crate so
//! conversion to/from any framework (Volga, Axum, Hyper, Actix) is
//! either zero-cost or a single `.into_parts()` call.

use bytes::Bytes;
use http::HeaderMap;

// The claims contract is public as `neva::auth::{Claims, DefaultClaims}`,
// without an HTTP transport too; this path is kept for engines that import it
// from here.
pub use crate::auth::{Claims, DefaultClaims};

/// Engine-neutral inbound HTTP request.
///
/// The body is fully buffered to [`Bytes`] before this type is constructed --
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

/// Outcome of a streaming-capable handler: an SSE stream or a complete
/// (single-body) reply.
///
/// This is the neutral shape of every MCP HTTP reply. Streamable HTTP has
/// allowed both forms on `POST` since 2025-03-26: a single JSON body (one
/// object, or an array for a batch) or a `text/event-stream` carrying
/// request-scoped messages. The GET handler uses the same shape for the
/// session SSE stream on the legacy transport.
///
/// `Stream` is the streaming path -- 200 OK + the event stream.
/// `Complete` is a finished single-body reply: a JSON object or batch array,
/// a `202 Accepted`, or an error status.
#[derive(Debug)]
pub enum StreamResponse<S> {
    /// 200 OK with an SSE event stream.
    Stream {
        /// Response headers (typically just `Mcp-Session-Id`).
        headers: HeaderMap,
        /// Stream of engine-native SSE event values.
        stream: S,
    },
    /// A complete non-streaming reply (JSON body or bare status).
    Complete(HttpResponse),
}

/// Former name of [`StreamResponse`], kept for one release.
///
/// Note the `Status` variant is now [`StreamResponse::Complete`].
#[deprecated(note = "renamed to StreamResponse; the Status variant is now Complete")]
pub type SseResponse<S> = StreamResponse<S>;

/// Identifier of one tracked SSE event: which stream carries it, and where in
/// that stream it sits.
///
/// This is what an engine writes into the `id:` field of an SSE frame, and
/// what a client hands back in `Last-Event-ID` to resume. It renders as
/// `<stream>:<seq>`.
///
/// The stream is part of the id rather than implied by the session. The
/// Streamable HTTP spec asks for cursors assigned "on a per-stream basis, to
/// act as a cursor within that particular stream", and forbids replaying on
/// one stream what was delivered on another -- neither is answerable unless
/// the id names the stream it belongs to. A session may hold several streams
/// at once, so a resumption has to say which one it is resuming.
///
/// # Example
///
/// ```rust,ignore
/// use neva::transport::http::EventId;
///
/// fn tracked_event(id: EventId, msg: &Message) -> SseMessage {
///     SseMessage::new().id(id.to_string()).json(msg)
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventId {
    stream: u64,
    seq: u64,
}

impl EventId {
    /// Creates an id for event `seq` of `stream`.
    #[inline]
    pub(crate) fn new(stream: u64, seq: u64) -> Self {
        Self { stream, seq }
    }

    /// The stream this event was delivered on.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// assert_eq!(id.stream(), 0);
    /// ```
    #[inline]
    pub fn stream(&self) -> u64 {
        self.stream
    }

    /// The event's position within its stream -- the cursor a resumption of
    /// that stream picks up from.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// assert_eq!(id.seq(), 7);
    /// ```
    #[inline]
    pub fn seq(&self) -> u64 {
        self.seq
    }
}

impl std::fmt::Display for EventId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.stream, self.seq)
    }
}

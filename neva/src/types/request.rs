//! Represents a request from an MCP client

use super::{JSONRPC_VERSION, Message, ProgressToken};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fmt::{Debug, Formatter};

#[cfg(feature = "server")]
use crate::Context;

#[cfg(feature = "http-server")]
use {crate::auth::Claims, http::HeaderMap, std::sync::Arc};

#[cfg(feature = "tasks")]
use crate::types::RelatedTaskMetadata;

#[cfg(feature = "server")]
pub use from_request::FromRequest;
pub use request_id::RequestId;

#[cfg(feature = "server")]
mod from_request;
mod request_id;

/// A request in the JSON-RPC protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// JSON-RPC protocol version.
    ///
    /// > **Note:** always 2.0.
    pub jsonrpc: String,

    /// Request identifier. Must be a string or number and unique within the session.
    pub id: RequestId,

    /// Name of the method to invoke.
    pub method: String,

    /// Optional parameters for the method.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,

    /// Current MCP Session ID
    #[serde(skip)]
    pub session_id: Option<uuid::Uuid>,

    /// HTTP headers
    #[serde(skip)]
    #[cfg(feature = "http-server")]
    pub headers: HeaderMap,

    /// Authentication and Authorization claims attached to this request by
    /// the HTTP engine. Type-erased so any engine can supply its own
    /// [`Claims`]-implementing type.
    #[serde(skip)]
    #[cfg(feature = "http-server")]
    pub claims: Option<Arc<dyn Claims>>,
}

/// Provides metadata related to the request that provides additional protocol-level information.
///
/// > **Note:** This class contains properties that are used by the Model Context Protocol
/// > for features like progress tracking and other protocol-specific capabilities.
#[derive(Default, Clone, Deserialize, Serialize)]
pub struct RequestParamsMeta {
    /// An opaque token that will be attached to any subsequent progress notifications.
    ///
    /// > **Note:** The receiver is not obligated to provide these notifications.
    #[serde(rename = "progressToken", skip_serializing_if = "Option::is_none")]
    pub progress_token: Option<ProgressToken>,

    /// W3C Trace Context `traceparent` carrier, when set by the sender.
    ///
    /// Always present in the struct for source-compatibility across feature
    /// configurations. The semantic interpretation (W3C Trace Context, MCP
    /// 2026-07-28) is meaningful under MCP 2026-07-28; older peers
    /// silently ignore the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traceparent: Option<String>,

    /// W3C Trace Context `tracestate` carrier, when set by the sender.
    ///
    /// Companion to [`Self::traceparent`]; carries vendor-specific state
    /// alongside the parent identifier. Same source-compatibility rationale
    /// applies -- the field is unconditional and older peers ignore it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracestate: Option<String>,

    /// W3C Baggage carrier, when set by the sender.
    ///
    /// The third key the spec reserves for OpenTelemetry propagation
    /// alongside [`Self::traceparent`] / [`Self::tracestate`]; values follow
    /// the [W3C Baggage](https://www.w3.org/TR/baggage/) format. Same
    /// source-compatibility rationale as its companions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baggage: Option<String>,

    /// Client implementation info carried on every request under MCP
    /// 2026-07-28 (replaces the `initialize` handshake's `clientInfo`).
    ///
    /// Always present in the struct for source-compatibility across feature
    /// configurations, like the trace fields; only populated (and meaningful)
    /// under MCP 2026-07-28. Older peers ignore it.
    #[serde(
        rename = "io.modelcontextprotocol/clientInfo",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) client_info: Option<super::Implementation>,

    /// MRTR: the client's results for a prior `InputRequiredResult`.
    ///
    /// **Read-only, and only for peers older than 0.5.3.** The spec puts this
    /// on the params (`InputResponseRequestParams`), which is where neva now
    /// writes it and where [`Request::input_responses`] looks first; the field
    /// survives here so a request from a 0.5.x neva client is still understood.
    /// It is never serialized, so nothing this build sends can carry it.
    #[cfg(not(feature = "legacy-spec"))]
    #[serde(rename = "inputResponses", default, skip_serializing)]
    pub(crate) input_responses: Option<crate::types::mrtr::InputResponses>,

    /// MRTR: the opaque `requestState` echoed back from `InputRequiredResult`.
    ///
    /// Read-only for the same reason as [`Self::input_responses`]; see
    /// [`Request::state`].
    #[cfg(not(feature = "legacy-spec"))]
    #[serde(rename = "requestState", default, skip_serializing)]
    pub(crate) request_state: Option<String>,

    /// Request-scoped logging level (MCP 2026-07-28).
    ///
    /// The minimum severity the client wants to receive as
    /// `notifications/message` while the server handles this request. This
    /// replaces the removed global `logging/setLevel` handshake; the desired
    /// level now rides on the originating request's `_meta`. Deprecated in the
    /// 2026-07-28 draft together with the rest of the logging surface.
    #[cfg(not(feature = "legacy-spec"))]
    #[serde(
        rename = "io.modelcontextprotocol/logLevel",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) log_level: Option<crate::types::notification::LoggingLevel>,

    /// The MCP protocol version this request is made under (MCP 2026-07-28).
    ///
    /// Required by the spec on every request. A version the server does not
    /// support draws
    /// [`ErrorCode::UnsupportedProtocolVersion`](crate::error::ErrorCode::UnsupportedProtocolVersion)
    /// on any transport -- see [`Request::unsupported_version_error`]. Over
    /// HTTP it must additionally match the `MCP-Protocol-Version` header, and
    /// since that header is checked before the body is read, a value that
    /// disagrees with it is by construction one the server does not support.
    ///
    /// Modelled as `Option` so a legacy-shaped request still parses -- the
    /// server treats an absent value as "not stated" rather than rejecting the
    /// parse.
    #[cfg(not(feature = "legacy-spec"))]
    #[serde(
        rename = "io.modelcontextprotocol/protocolVersion",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) protocol_version: Option<String>,

    /// MRTR/stateless: client capabilities declared per-request (v1: a single
    /// `elicitation` flag) so the server can honor "MUST NOT send an input
    /// type the client didn't declare".
    #[cfg(not(feature = "legacy-spec"))]
    #[serde(
        rename = "io.modelcontextprotocol/clientCapabilities",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) client_capabilities: Option<crate::types::mrtr::ClientMrtrCapabilities>,

    /// Represents metadata for associating messages with a task.
    ///
    /// > **Note:** Include this in the _meta field under the key `io.modelcontextprotocol/related-task`.
    #[serde(
        rename = "io.modelcontextprotocol/related-task",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg(feature = "tasks")]
    pub(crate) task: Option<RelatedTaskMetadata>,

    /// MCP request context
    #[serde(skip)]
    #[cfg(feature = "server")]
    pub(crate) context: Option<Context>,
}

impl Debug for RequestParamsMeta {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestParamsMeta")
            .field("progress_token", &self.progress_token)
            .field("traceparent", &self.traceparent)
            .field("tracestate", &self.tracestate)
            .finish()
    }
}

impl From<Request> for Message {
    #[inline]
    fn from(request: Request) -> Self {
        Self::Request(request)
    }
}

impl RequestParamsMeta {
    /// Creates a new [`RequestParamsMeta`] with [`ProgressToken`] for a specific [`RequestId`]
    pub fn new(id: &RequestId) -> Self {
        Self {
            progress_token: Some(ProgressToken::from(id)),
            ..Default::default()
        }
    }
}

impl Request {
    /// Creates a new [`Request`]
    pub fn new<T: Serialize>(
        id: Option<RequestId>,
        method: impl Into<String>,
        params: Option<T>,
    ) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            session_id: None,
            id: id.unwrap_or_default(),
            method: method.into(),
            params: params.and_then(|p| serde_json::to_value(p).ok()),
            #[cfg(feature = "http-server")]
            headers: HeaderMap::with_capacity(8),
            #[cfg(feature = "http-server")]
            claims: None,
        }
    }

    /// Returns request's id if it's specified, otherwise returns default value
    ///
    /// Default: `(no id)`
    pub fn id(&self) -> RequestId {
        self.id.clone()
    }

    /// Why this request's `_meta` is not acceptable under MCP 2026-07-28, if it
    /// is not.
    ///
    /// `io.modelcontextprotocol/protocolVersion` (a string) and
    /// `io.modelcontextprotocol/clientCapabilities` (an object) are required on
    /// every request -- capabilities are declared per request precisely so a
    /// stateless server never has to infer them from earlier traffic -- and a
    /// request that omits either, or states it with the wrong JSON type, is
    /// malformed params.
    ///
    /// This is a property of the message, not of how it arrived, so it belongs
    /// to the request rather than to a transport: a stdio server owes the same
    /// rejection an HTTP one does. The HTTP layer additionally checks the
    /// stated version against the `MCP-Protocol-Version` header and answers
    /// `400`, neither of which means anything off that transport.
    ///
    /// # Examples
    /// ```
    /// use neva::types::Request;
    ///
    /// let bare = Request::new(None, "tools/list", None::<()>);
    /// assert!(bare.required_meta_error().is_some());
    ///
    /// let complete = Request::new(None, "tools/list", Some(serde_json::json!({
    ///     "_meta": {
    ///         "io.modelcontextprotocol/protocolVersion": "2026-07-28",
    ///         "io.modelcontextprotocol/clientCapabilities": {}
    ///     }
    /// })));
    /// assert!(complete.required_meta_error().is_none());
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub fn required_meta_error(&self) -> Option<crate::error::Error> {
        use crate::error::{Error, ErrorCode};

        const VERSION: &str = "io.modelcontextprotocol/protocolVersion";
        const CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";

        let meta = self.params.as_ref().and_then(|p| p.get("_meta"));
        let malformed = |key: &str, expected: &str| {
            Some(Error::new(
                ErrorCode::InvalidParams,
                format!("request `_meta` is missing the required `{key}` {expected}"),
            ))
        };

        if meta
            .and_then(|m| m.get(VERSION))
            .and_then(|v| v.as_str())
            .is_none()
        {
            return malformed(VERSION, "string");
        }
        if meta
            .and_then(|m| m.get(CAPABILITIES))
            .is_none_or(|v| !v.is_object())
        {
            return malformed(CAPABILITIES, "object");
        }
        None
    }

    /// The protocol version this request states in its `_meta`, if it states a
    /// well-formed one.
    ///
    /// # Examples
    /// ```
    /// use neva::types::Request;
    ///
    /// let req = Request::new(None, "tools/list", Some(serde_json::json!({
    ///     "_meta": { "io.modelcontextprotocol/protocolVersion": "2026-07-28" }
    /// })));
    /// assert_eq!(req.stated_protocol_version(), Some("2026-07-28"));
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub fn stated_protocol_version(&self) -> Option<&str> {
        self.params
            .as_ref()?
            .get("_meta")?
            .get("io.modelcontextprotocol/protocolVersion")?
            .as_str()
    }

    /// The MRTR answers this request carries, if any.
    ///
    /// The spec puts `inputResponses` on the params themselves
    /// (`InputResponseRequestParams`), next to `name` / `arguments` -- not in
    /// `_meta`. neva wrote them into `_meta` up to 0.5.2, so a request is read
    /// from the spec location first and from the old one only as a fallback:
    /// that keeps a 0.5.x client talking to a newer server, and costs one
    /// lookup on a request that has neither.
    ///
    /// # Examples
    /// ```
    /// use neva::types::Request;
    ///
    /// let req = Request::new(None, "tools/call", Some(serde_json::json!({
    ///     "name": "greet",
    ///     "inputResponses": { "who": { "action": "accept" } }
    /// })));
    /// assert!(req.input_responses().is_some());
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub fn input_responses(&self) -> Option<crate::types::mrtr::InputResponses> {
        let from_params = self
            .params
            .as_ref()
            .and_then(|p| p.get("inputResponses"))
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        from_params.or_else(|| self.meta().and_then(|m| m.input_responses))
    }

    /// The opaque MRTR `requestState` this request echoes back, if any.
    ///
    /// Named for the receiver rather than the wire: `req.state()` says what
    /// `req.request_state()` would, without the stutter. The field it reads is
    /// `requestState`, in either of the two places below.
    ///
    /// Same two locations, same order, and the same reason as
    /// [`Self::input_responses`].
    ///
    /// # Examples
    /// ```
    /// use neva::types::Request;
    ///
    /// let req = Request::new(None, "tools/call", Some(serde_json::json!({
    ///     "name": "greet",
    ///     "requestState": "v1.0.sealed"
    /// })));
    /// assert_eq!(req.state().as_deref(), Some("v1.0.sealed"));
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub fn state(&self) -> Option<String> {
        let from_params = self
            .params
            .as_ref()
            .and_then(|p| p.get("requestState"))
            .and_then(|v| v.as_str())
            .map(str::to_owned);

        from_params.or_else(|| self.meta().and_then(|m| m.request_state))
    }

    /// Why this request's MRTR fields are not acceptable, if they are not.
    ///
    /// [`Self::state`] and [`Self::input_responses`] both answer `None` for a
    /// field that is present but of the wrong JSON type, because neither can
    /// say anything else. Left at that, a `requestState` of the wrong type
    /// would read as *no state at all*: the retry would take the absent-state
    /// path, its `inputResponses` would be treated as answers offered up front
    /// rather than as the continuation of a round, and the client would be told
    /// nothing about why. Worse, with the field malformed in one of the two
    /// locations and well-formed in the other, the accessors would quietly read
    /// the other one.
    ///
    /// So the accessors keep their `Option`, and stating a recognized field
    /// with the wrong type is rejected here instead -- a property of the
    /// message, checked where [`Self::required_meta_error`] is.
    ///
    /// # Examples
    /// ```
    /// use neva::types::Request;
    ///
    /// let good = Request::new(None, "tools/call", Some(serde_json::json!({
    ///     "name": "greet",
    ///     "requestState": "v1.0.sealed"
    /// })));
    /// assert!(good.malformed_mrtr_error().is_none());
    ///
    /// let bad = Request::new(None, "tools/call", Some(serde_json::json!({
    ///     "name": "greet",
    ///     "requestState": 42
    /// })));
    /// assert!(bad.malformed_mrtr_error().is_some());
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub fn malformed_mrtr_error(&self) -> Option<crate::error::Error> {
        use crate::error::{Error, ErrorCode};

        let params = self.params.as_ref()?;
        let malformed = |key: &str, expected: &str| {
            Some(Error::new(
                ErrorCode::InvalidParams,
                format!("`{key}` must be {expected}"),
            ))
        };

        // `null` counts as stating the field, and stating it wrong. The spec
        // types these as `string` and `object` and makes them optional by
        // *absence* -- an omitted property, not a null one -- so `null` is a
        // value of the wrong type like any other. Excusing it would leave the
        // hole this check exists to close: a retry meaning to continue a chain
        // would read as starting fresh, and its answers be taken as offered up
        // front.
        //
        // Both locations the accessors read, in the same order and for the same
        // reason: a 0.5.x peer states these in `_meta`, a current one on the
        // params, and either may be the malformed one.
        for source in [Some(params), params.get("_meta")].into_iter().flatten() {
            if source.get("requestState").is_some_and(|v| !v.is_string()) {
                return malformed("requestState", "a string");
            }
            if source.get("inputResponses").is_some_and(|v| !v.is_object()) {
                return malformed("inputResponses", "an object");
            }
        }
        None
    }

    /// Why the protocol version this request states is one this build cannot
    /// serve, if it is.
    ///
    /// `_meta.io.modelcontextprotocol/protocolVersion` names the version the
    /// request is made under, and a server that does not speak it must answer
    /// `UnsupportedProtocolVersion` (`-32022`) carrying what it does speak, so
    /// the caller can pick from that list and retry.
    ///
    /// Like [`Self::required_meta_error`], this is a property of the message
    /// and not of how it arrived: the version is stated in the body, so a stdio
    /// server owes the same rejection an HTTP one does. What is transport
    /// specific is only the `400` HTTP additionally mandates for it.
    ///
    /// A request stating no well-formed version has nothing to compare and is
    /// [`Self::required_meta_error`]'s to reject.
    ///
    /// # Examples
    /// ```
    /// use neva::types::Request;
    ///
    /// let stale = Request::new(None, "tools/list", Some(serde_json::json!({
    ///     "_meta": { "io.modelcontextprotocol/protocolVersion": "2025-06-18" }
    /// })));
    /// let err = stale.unsupported_version_error().expect("not served");
    /// // The client is told what is on offer, not merely that it guessed wrong.
    /// assert_eq!(err.data().unwrap()["requested"], "2025-06-18");
    ///
    /// let current = Request::new(None, "tools/list", Some(serde_json::json!({
    ///     "_meta": { "io.modelcontextprotocol/protocolVersion": "2026-07-28" }
    /// })));
    /// assert!(current.unsupported_version_error().is_none());
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub fn unsupported_version_error(&self) -> Option<crate::error::Error> {
        use crate::error::{Error, ErrorCode};

        let stated = self.stated_protocol_version()?;
        (stated != crate::LATEST_PROTOCOL_VERSION).then(|| {
            Error::new(
                ErrorCode::UnsupportedProtocolVersion,
                format!("Unsupported MCP protocol version: {stated}"),
            )
            .with_data(serde_json::json!({
                "supported": [crate::LATEST_PROTOCOL_VERSION],
                "requested": stated,
            }))
        })
    }

    /// Returns the full id (session_id?/request_id)
    pub fn full_id(&self) -> RequestId {
        let id = self.id.clone();
        if let Some(session_id) = self.session_id {
            id.concat(RequestId::Uuid(session_id))
        } else {
            id
        }
    }

    /// Returns [`Request`] params metadata
    pub fn meta(&self) -> Option<RequestParamsMeta> {
        self.params
            .as_ref()?
            .get("_meta")
            .cloned()
            .and_then(|meta| serde_json::from_value(meta).ok())
    }

    /// Merges `meta` into the request's `_meta`, creating the params/`_meta`
    /// objects when none exist. Symmetric counterpart to [`Self::meta`];
    /// existing (non-`_meta`) params keys are preserved, as are any `_meta`
    /// entries the typed [`RequestParamsMeta`] does not model -- e.g. custom
    /// extension keys such as `com.example/foo` -- which a full replacement
    /// would silently drop. Only the fields populated on `meta` are written;
    /// unset (`None`) fields leave any existing entry untouched.
    ///
    /// Non-object params (a scalar or array payload, e.g. from
    /// `command("x", Some(vec![1, 2]))`) are left untouched: `_meta` has no
    /// place on a non-object JSON-RPC params value, and replacing it would
    /// silently drop the caller's payload, so metadata injection is skipped.
    #[cfg(all(feature = "client", not(feature = "legacy-spec")))]
    pub(crate) fn set_meta(&mut self, meta: RequestParamsMeta) {
        let Ok(serde_json::Value::Object(fields)) = serde_json::to_value(meta) else {
            return;
        };
        match self.params {
            Some(serde_json::Value::Object(ref mut map)) => match map.get_mut("_meta") {
                Some(serde_json::Value::Object(existing)) => existing.extend(fields),
                _ => {
                    map.insert("_meta".to_owned(), serde_json::Value::Object(fields));
                }
            },
            // No params yet: create the params object carrying just `_meta`.
            None => {
                let mut map = serde_json::Map::new();
                map.insert("_meta".to_owned(), serde_json::Value::Object(fields));
                self.params = Some(serde_json::Value::Object(map));
            }
            // Non-object params: preserve the caller's payload rather than
            // overwriting it; `_meta` cannot be attached to a scalar/array.
            Some(_) => {
                #[cfg(feature = "tracing")]
                tracing::debug!(
                    logger = "neva",
                    "skipping client _meta injection: request params are not a JSON object"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_context_roundtrips_through_meta() {
        use serde_json::json;
        let meta = RequestParamsMeta {
            traceparent: Some("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".into()),
            tracestate: Some("congo=t61rcWkgMzE".into()),
            ..Default::default()
        };
        let v = serde_json::to_value(&meta).unwrap();
        assert_eq!(
            v["traceparent"],
            json!("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01")
        );
        assert_eq!(v["tracestate"], json!("congo=t61rcWkgMzE"));
        let back: RequestParamsMeta = serde_json::from_value(v).unwrap();
        assert_eq!(back.traceparent.as_deref(), meta.traceparent.as_deref());
        assert_eq!(back.tracestate.as_deref(), meta.tracestate.as_deref());
    }

    /// The version is stated in the body, so the rule holds on every transport
    /// -- a stdio server reaches it through the dispatch seam, which is the
    /// only gate it has.
    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn a_version_this_build_does_not_serve_is_refused() {
        use crate::error::ErrorCode;
        use serde_json::json;

        let with_version = |v: serde_json::Value| {
            Request::new(
                None,
                "tools/list",
                Some(json!({ "_meta": {
                    "io.modelcontextprotocol/protocolVersion": v,
                    "io.modelcontextprotocol/clientCapabilities": {}
                } })),
            )
        };

        let err = with_version(json!("2025-06-18"))
            .unsupported_version_error()
            .expect("a version this build does not speak");
        assert_eq!(err.code, ErrorCode::UnsupportedProtocolVersion);
        let data = err.data().expect("the retry data the spec specifies");
        assert_eq!(data["requested"], "2025-06-18");
        assert_eq!(data["supported"], json!([crate::LATEST_PROTOCOL_VERSION]));

        assert!(
            with_version(json!(crate::LATEST_PROTOCOL_VERSION))
                .unsupported_version_error()
                .is_none()
        );
        // Not a string is not a version: `required_meta_error` owns that, and
        // this one must not double-report it as unsupported.
        assert!(
            with_version(json!(2026))
                .unsupported_version_error()
                .is_none()
        );
        assert!(
            Request::new(None, "tools/list", None::<()>)
                .unsupported_version_error()
                .is_none()
        );
    }

    #[test]
    fn meta_without_trace_context_omits_fields() {
        let meta = RequestParamsMeta::default();
        let v = serde_json::to_value(&meta).unwrap();
        assert!(v.get("traceparent").is_none());
        assert!(v.get("tracestate").is_none());
    }

    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn log_level_roundtrips_under_spec_meta_key() {
        use crate::types::notification::LoggingLevel;
        use serde_json::json;

        let meta = RequestParamsMeta {
            log_level: Some(LoggingLevel::Warning),
            ..Default::default()
        };
        let v = serde_json::to_value(&meta).unwrap();
        // The request-scoped level rides under the spec `_meta` key, lowercase.
        assert_eq!(v["io.modelcontextprotocol/logLevel"], json!("warning"));

        let back: RequestParamsMeta = serde_json::from_value(v).unwrap();
        assert_eq!(back.log_level, Some(LoggingLevel::Warning));
    }

    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn absent_log_level_is_omitted() {
        let meta = RequestParamsMeta::default();
        let v = serde_json::to_value(&meta).unwrap();
        assert!(v.get("io.modelcontextprotocol/logLevel").is_none());
    }

    #[cfg(all(feature = "client", not(feature = "legacy-spec")))]
    #[test]
    fn set_meta_writes_meta_and_preserves_params() {
        use serde_json::json;
        let mut req = Request::new(Some(RequestId::Number(1)), "ping", Some(json!({ "x": 1 })));
        let meta = RequestParamsMeta {
            traceparent: Some("tp".into()),
            client_info: Some(crate::types::Implementation {
                name: "c".into(),
                version: "9".into(),
                icons: None,
            }),
            ..Default::default()
        };
        req.set_meta(meta);

        // _meta round-trips through the typed struct, preserving siblings.
        let got = req.meta().expect("meta present");
        assert_eq!(got.traceparent.as_deref(), Some("tp"));
        assert_eq!(got.client_info.expect("client_info present").name, "c");
        // MRTR meta fields default to None and survive set/get.
        assert!(got.input_responses.is_none());
        assert!(got.request_state.is_none());
        // pre-existing params keys are untouched.
        assert_eq!(req.params.expect("params present")["x"], json!(1));
    }

    /// The MRTR re-run fields are params. `_meta` is read only as a fallback,
    /// for a 0.5.x neva client, and the params win when both are present.
    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn mrtr_fields_are_read_from_params_first() {
        use serde_json::json;

        let spec = Request::new(
            None,
            "tools/call",
            Some(json!({
                "name": "greet",
                "requestState": "from-params",
                "inputResponses": { "who": { "action": "accept" } }
            })),
        );
        assert_eq!(spec.state().as_deref(), Some("from-params"));
        assert!(spec.input_responses().expect("answers").contains_key("who"));

        let legacy = Request::new(
            None,
            "tools/call",
            Some(json!({
                "name": "greet",
                "_meta": {
                    "requestState": "from-meta",
                    "inputResponses": { "who": { "action": "accept" } }
                }
            })),
        );
        assert_eq!(legacy.state().as_deref(), Some("from-meta"));
        assert!(
            legacy
                .input_responses()
                .expect("answers")
                .contains_key("who")
        );

        let both = Request::new(
            None,
            "tools/call",
            Some(json!({
                "name": "greet",
                "requestState": "from-params",
                "_meta": { "requestState": "from-meta" }
            })),
        );
        assert_eq!(both.state().as_deref(), Some("from-params"));

        let neither = Request::new(None, "tools/call", Some(json!({ "name": "greet" })));
        assert!(neither.state().is_none());
        assert!(neither.input_responses().is_none());
    }

    /// A recognized field stated with the wrong JSON type is malformed params,
    /// not an absent field: read as absent, a bad `requestState` turns a retry
    /// into a fresh call whose answers look like ones offered up front.
    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn a_mrtr_field_of_the_wrong_type_is_rejected() {
        use serde_json::json;

        let call = |params| Request::new(None, "tools/call", Some(params));

        for params in [
            json!({ "name": "greet", "requestState": 42 }),
            json!({ "name": "greet", "requestState": { "sealed": true } }),
            json!({ "name": "greet", "inputResponses": "who" }),
            json!({ "name": "greet", "inputResponses": ["who"] }),
            // The old location is read too, so it is checked too.
            json!({ "name": "greet", "_meta": { "requestState": 42 } }),
            json!({ "name": "greet", "_meta": { "inputResponses": 7 } }),
            // Malformed where the accessors look first, well-formed where they
            // look second: without this check the request would be served
            // against a state it did not state here.
            json!({
                "name": "greet",
                "requestState": 42,
                "_meta": { "requestState": "from-meta" }
            }),
            // `null` states the field, and states it wrong: the spec makes
            // these optional by absence, so a peer with nothing to say leaves
            // them out. Excusing `null` would read a retry as a fresh call and
            // take its answers as offered up front.
            json!({ "name": "greet", "requestState": null }),
            json!({ "name": "greet", "inputResponses": null }),
        ] {
            assert!(
                call(params.clone()).malformed_mrtr_error().is_some(),
                "{params} must be rejected"
            );
        }

        for params in [
            json!({ "name": "greet" }),
            json!({ "name": "greet", "requestState": "v1.0.sealed" }),
            json!({
                "name": "greet",
                "requestState": "v1.0.sealed",
                "inputResponses": { "who": { "action": "accept" } }
            }),
        ] {
            assert!(
                call(params.clone()).malformed_mrtr_error().is_none(),
                "{params} must be accepted"
            );
        }

        assert!(
            Request::new(None, "tools/list", None::<()>)
                .malformed_mrtr_error()
                .is_none(),
            "a request without params states no MRTR fields at all"
        );
    }

    /// Nothing this build sends may carry the old location, or a peer reading
    /// the spec one would see an answered retry as a fresh call.
    #[cfg(not(feature = "legacy-spec"))]
    #[test]
    fn mrtr_meta_fields_are_never_written() {
        let meta = RequestParamsMeta {
            request_state: Some("sealed".into()),
            input_responses: Some(
                [("who".to_string(), serde_json::json!({ "action": "accept" }))]
                    .into_iter()
                    .collect(),
            ),
            ..Default::default()
        };

        let json = serde_json::to_value(&meta).expect("serialize");
        assert!(json.get("requestState").is_none(), "got: {json}");
        assert!(json.get("inputResponses").is_none(), "got: {json}");
    }

    #[cfg(all(feature = "client", not(feature = "legacy-spec")))]
    #[test]
    fn set_meta_preserves_unknown_meta_entries() {
        use serde_json::json;
        // A caller-supplied `_meta` carrying a custom extension key the typed
        // `RequestParamsMeta` does not model.
        let mut req = Request::new(
            Some(RequestId::Number(1)),
            "tools/call",
            Some(json!({ "name": "echo", "_meta": { "com.example/foo": 1 } })),
        );
        let meta = RequestParamsMeta {
            client_info: Some(crate::types::Implementation {
                name: "c".into(),
                version: "9".into(),
                icons: None,
            }),
            ..Default::default()
        };
        req.set_meta(meta);

        let params = req.params.expect("params present");
        // Custom extension key survives the merge.
        assert_eq!(params["_meta"]["com.example/foo"], json!(1));
        // Newly applied client field is present alongside it.
        assert_eq!(
            params["_meta"]["io.modelcontextprotocol/clientInfo"]["name"],
            json!("c")
        );
        // Sibling params keys are untouched.
        assert_eq!(params["name"], json!("echo"));
    }

    #[cfg(all(feature = "client", not(feature = "legacy-spec")))]
    #[test]
    fn set_meta_preserves_non_object_params() {
        use serde_json::json;
        // A custom command with an array payload, e.g. command("x", Some(vec![1, 2])).
        let mut req = Request::new(Some(RequestId::Number(1)), "x", Some(json!([1, 2])));
        let meta = RequestParamsMeta {
            client_info: Some(crate::types::Implementation {
                name: "c".into(),
                version: "9".into(),
                icons: None,
            }),
            ..Default::default()
        };
        req.set_meta(meta);

        // The array payload is preserved verbatim; no `_meta` object is grafted on.
        assert_eq!(req.params, Some(json!([1, 2])));

        // Same for a scalar payload.
        let mut req = Request::new(Some(RequestId::Number(2)), "x", Some(json!("id")));
        req.set_meta(RequestParamsMeta::default());
        assert_eq!(req.params, Some(json!("id")));
    }

    #[cfg(all(feature = "client", not(feature = "legacy-spec")))]
    #[test]
    fn set_meta_creates_params_when_absent() {
        let mut req = Request::new(Some(RequestId::Number(1)), "x", None::<serde_json::Value>);
        let meta = RequestParamsMeta {
            client_info: Some(crate::types::Implementation {
                name: "c".into(),
                version: "9".into(),
                icons: None,
            }),
            ..Default::default()
        };
        req.set_meta(meta);

        let got = req.meta().expect("meta present");
        assert_eq!(got.client_info.expect("client_info present").name, "c");
    }
}

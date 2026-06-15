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
    /// 2026-07-28) is meaningful under `proto-2026-07-28-rc`; older peers
    /// silently ignore the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traceparent: Option<String>,

    /// W3C Trace Context `tracestate` carrier, when set by the sender.
    ///
    /// Companion to [`Self::traceparent`]; carries vendor-specific state
    /// alongside the parent identifier. Same source-compatibility rationale
    /// applies — the field is unconditional and older peers ignore it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracestate: Option<String>,

    /// Client implementation info carried on every request under MCP
    /// 2026-07-28 (replaces the `initialize` handshake's `clientInfo`).
    ///
    /// Always present in the struct for source-compatibility across feature
    /// configurations, like the trace fields; only populated (and meaningful)
    /// under `proto-2026-07-28-rc`. Older peers ignore it.
    #[serde(
        rename = "io.modelcontextprotocol/clientInfo",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) client_info: Option<super::Implementation>,

    /// MRTR: the client's results for a prior `InputRequiredResult`.
    #[cfg(feature = "proto-2026-07-28-rc")]
    #[serde(rename = "inputResponses", skip_serializing_if = "Option::is_none")]
    pub(crate) input_responses: Option<crate::types::mrtr::InputResponses>,

    /// MRTR: the opaque `requestState` echoed back from `InputRequiredResult`.
    #[cfg(feature = "proto-2026-07-28-rc")]
    #[serde(rename = "requestState", skip_serializing_if = "Option::is_none")]
    pub(crate) request_state: Option<String>,

    /// MRTR/stateless: client capabilities declared per-request (v1: a single
    /// `elicitation` flag) so the server can honor "MUST NOT send an input
    /// type the client didn't declare".
    #[cfg(feature = "proto-2026-07-28-rc")]
    #[serde(rename = "clientCapabilities", skip_serializing_if = "Option::is_none")]
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
    /// entries the typed [`RequestParamsMeta`] does not model — e.g. custom
    /// extension keys such as `com.example/foo` — which a full replacement
    /// would silently drop. Only the fields populated on `meta` are written;
    /// unset (`None`) fields leave any existing entry untouched.
    #[cfg(all(feature = "client", feature = "proto-2026-07-28-rc"))]
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
            _ => {
                let mut map = serde_json::Map::new();
                map.insert("_meta".to_owned(), serde_json::Value::Object(fields));
                self.params = Some(serde_json::Value::Object(map));
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

    #[test]
    fn meta_without_trace_context_omits_fields() {
        let meta = RequestParamsMeta::default();
        let v = serde_json::to_value(&meta).unwrap();
        assert!(v.get("traceparent").is_none());
        assert!(v.get("tracestate").is_none());
    }

    #[cfg(all(feature = "client", feature = "proto-2026-07-28-rc"))]
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

    #[cfg(all(feature = "client", feature = "proto-2026-07-28-rc"))]
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
}

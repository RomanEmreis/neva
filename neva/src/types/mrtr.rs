//! Multi Round-Trip Request (MRTR) wire types (MCP `proto-2026-07-28-rc`).
//!
//! A server processing `tools/call` / `prompts/get` / `resources/read` may
//! reply with [`InputRequiredResult`] to request additional input (v1:
//! elicitation only) before completing. See
//! `docs/specs/2026-05-30-mrtr-design.md`.

// The encrypted `requestState` codec is server-only: the client treats
// `requestState` as opaque and never encodes/decodes it.
#[cfg(feature = "server")]
pub(crate) mod state;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::types::elicitation::{ElicitRequestParams, ElicitResult};
use crate::types::{IntoResponse, RequestId, Response};

/// A result indicating the server needs more input before it can complete the
/// request. Recognized for `tools/call`, `prompts/get`, `resources/read`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputRequiredResult {
    /// Discriminator, always `"input_required"`.
    #[serde(rename = "resultType")]
    pub result_type: InputRequiredTag,

    /// Server-assigned-key → elicitation request the client must fulfil.
    ///
    /// `None` is **reserved** for a future async/streaming semantic where the
    /// server is making progress on its own and the client should simply retry
    /// with the echoed [`Self::request_state`] (no new inputs to gather). That
    /// path is not implemented yet; today this is always `Some(..)`. Because the
    /// field is already `Option`, adding that behavior later is non-breaking.
    #[serde(rename = "inputRequests", skip_serializing_if = "Option::is_none")]
    pub input_requests: Option<InputRequests>,

    /// Opaque, server-meaningful state the client echoes back verbatim.
    #[serde(rename = "requestState", skip_serializing_if = "Option::is_none")]
    pub request_state: Option<String>,
}

/// Map of server-assigned key → elicitation request envelope.
pub type InputRequests = HashMap<String, ElicitationInputRequest>;

/// Map of key (matching an [`InputRequests`] key) → the client's result.
pub type InputResponses = HashMap<String, ElicitResult>;

/// The `{ method, params }` envelope for one elicitation input request.
///
/// Intentionally *not* [`crate::types::Request`]: the wire shape is exactly
/// `{ method, params }` (the per-key id is the map key), whereas `Request`
/// has required `jsonrpc`/`id` fields — emitting it would add non-spec fields
/// and deserializing a conformant peer's bare `{method,params}` would fail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationInputRequest {
    /// Always `"elicitation/create"`.
    pub method: ElicitationCreateMethod,
    /// The elicitation parameters.
    pub params: ElicitRequestParams,
}

/// Per-request client capability flags relevant to MRTR (v1: elicitation).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ClientMrtrCapabilities {
    /// Whether the client can fulfil `elicitation/create` input requests.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub elicitation: bool,
}

/// Unit tag serializing as the constant string `"input_required"`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum InputRequiredTag {
    /// The only variant.
    #[serde(rename = "input_required")]
    InputRequired,
}

/// Unit tag serializing as the constant string `"elicitation/create"`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ElicitationCreateMethod {
    /// The only variant.
    #[serde(rename = "elicitation/create")]
    ElicitationCreate,
}

// Server-only: only the server constructs `InputRequiredResult`; the client
// deserializes it from the wire.
#[cfg(feature = "server")]
impl InputRequiredResult {
    /// Builds an `InputRequiredResult` for a single elicitation request.
    pub(crate) fn elicitation(key: String, params: ElicitRequestParams, state: String) -> Self {
        let mut input_requests = HashMap::with_capacity(1);
        input_requests.insert(
            key,
            ElicitationInputRequest {
                method: ElicitationCreateMethod::ElicitationCreate,
                params,
            },
        );
        Self {
            result_type: InputRequiredTag::InputRequired,
            input_requests: Some(input_requests),
            request_state: Some(state),
        }
    }
}

impl IntoResponse for InputRequiredResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        match serde_json::to_value(self) {
            Ok(v) => Response::success(req_id, v),
            Err(err) => Response::error(req_id, err.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_required_result_roundtrips_with_tag_and_envelope() {
        let json = r#"{
            "resultType": "input_required",
            "inputRequests": {
                "ask_name": {
                    "method": "elicitation/create",
                    "params": { "Form": {
                        "message": "Your name?",
                        "mode": null,
                        "requestedSchema": { "type": "object", "properties": {}, "required": null }
                    }}
                }
            },
            "requestState": "abc.def"
        }"#;
        let parsed: InputRequiredResult = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.request_state.as_deref(), Some("abc.def"));
        assert!(
            parsed
                .input_requests
                .as_ref()
                .expect("requests")
                .contains_key("ask_name")
        );
        let back = serde_json::to_value(&parsed).unwrap();
        assert_eq!(back["resultType"], serde_json::json!("input_required"));
        assert_eq!(
            back["inputRequests"]["ask_name"]["method"],
            serde_json::json!("elicitation/create")
        );
    }
}

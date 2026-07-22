//! Multi Round-Trip Request (MRTR) wire types (MCP `proto-2026-07-28-rc`).
//!
//! A server processing `tools/call` / `prompts/get` / `resources/read` may
//! reply with [`InputRequiredResult`] to request additional input before
//! completing. The kind of input is the [`InputRequest`] union: elicitation
//! (first-class), plus the deprecated-on-arrival sampling and roots kinds the
//! spec re-homed here when it removed their capability-driven server→client
//! requests. See `docs/specs/2026-05-30-mrtr-design.md`.

// The encrypted `requestState` codec is server-only: the client treats
// `requestState` as opaque and never encodes/decodes it.
#[cfg(feature = "server")]
pub(crate) mod state;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::types::elicitation::ElicitRequestParams;
use crate::types::root::ListRootsRequestParams;
use crate::types::sampling::CreateMessageRequestParams;
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

/// Map of server-assigned key → the input request envelope the client must
/// fulfil.
pub type InputRequests = HashMap<String, InputRequest>;

/// Map of key (matching an [`InputRequests`] key) → the client's raw result.
///
/// The value stays a [`serde_json::Value`] because the result *type* depends on
/// the kind of input that was requested ([`ElicitResult`](crate::types::elicitation::ElicitResult),
/// [`CreateMessageResult`](crate::types::sampling::CreateMessageResult),
/// [`ListRootsResult`](crate::types::root::ListRootsResult)). Each server-side
/// helper deserializes its own type out of the replay log, exactly like
/// [`Context::memo`](crate::Context::memo) does.
pub type InputResponses = HashMap<String, serde_json::Value>;

/// One `{ method, params }` input-request envelope — the kind of input the
/// server is asking the client for.
///
/// The spec did not delete sampling and roots when the capability-driven
/// server→client requests went away: it re-homed them here, as MRTR input
/// request kinds, keyed by `method`. Elicitation stays first-class; the other
/// two arrive **already deprecated** (see the variant docs), matching the
/// spec's own 12-month lifecycle for roots/sampling/logging.
///
/// Intentionally *not* [`crate::types::Request`]: the wire shape is exactly
/// `{ method, params }` (the per-key id is the map key), whereas `Request`
/// has required `jsonrpc`/`id` fields — emitting it would add non-spec fields
/// and deserializing a conformant peer's bare `{method,params}` would fail.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum InputRequest {
    /// `elicitation/create` — ask the end user for structured input.
    #[serde(rename = "elicitation/create")]
    Elicitation(ElicitRequestParams),

    /// `sampling/createMessage` — ask the client's LLM for a completion.
    ///
    /// **Deprecated on arrival.** The ability returns re-homed onto MRTR, but
    /// it stays on the spec's deprecation path; prefer designing tools that do
    /// not need the client's model.
    #[serde(rename = "sampling/createMessage")]
    #[deprecated(
        note = "sampling is deprecated in MCP 2026-07-28; it returns as an MRTR input-request kind only for migration"
    )]
    Sampling(Box<CreateMessageRequestParams>),

    /// `roots/list` — ask the client which filesystem roots it exposes.
    ///
    /// **Deprecated on arrival**, same as [`Self::Sampling`].
    #[serde(rename = "roots/list")]
    #[deprecated(
        note = "roots are deprecated in MCP 2026-07-28; they return as an MRTR input-request kind only for migration"
    )]
    Roots(Box<ListRootsRequestParams>),
}

impl InputRequest {
    /// The JSON-RPC method name this envelope carries.
    pub fn method(&self) -> &'static str {
        #[allow(deprecated)]
        match self {
            Self::Elicitation(_) => crate::types::elicitation::commands::CREATE,
            Self::Sampling(_) => crate::types::sampling::commands::CREATE,
            Self::Roots(_) => crate::types::root::commands::LIST,
        }
    }
}

/// Per-request client capability flags relevant to MRTR.
///
/// The server gates each input-request kind on the matching flag: asking for an
/// input the client never declared is a server bug, and is reported as such
/// rather than stalling the round-trip.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ClientMrtrCapabilities {
    /// Whether the client can fulfil `elicitation/create` input requests.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub elicitation: bool,

    /// Whether the client can fulfil `sampling/createMessage` input requests.
    ///
    /// A **deprecated** request kind — see [`InputRequest::Sampling`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sampling: bool,

    /// Whether the client can fulfil `roots/list` input requests.
    ///
    /// A **deprecated** request kind — see [`InputRequest::Roots`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub roots: bool,
}

#[cfg(feature = "server")]
impl ClientMrtrCapabilities {
    /// Whether the client declared support for the kind `request` asks for.
    pub(crate) fn allows(&self, request: &InputRequest) -> bool {
        #[allow(deprecated)]
        match request {
            InputRequest::Elicitation(_) => self.elicitation,
            InputRequest::Sampling(_) => self.sampling,
            InputRequest::Roots(_) => self.roots,
        }
    }
}

/// Unit tag serializing as the constant string `"input_required"`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum InputRequiredTag {
    /// The only variant.
    #[serde(rename = "input_required")]
    InputRequired,
}

// Server-only: only the server constructs `InputRequiredResult`; the client
// deserializes it from the wire.
#[cfg(feature = "server")]
impl InputRequiredResult {
    /// Builds an `InputRequiredResult` for a single input request of any kind.
    pub(crate) fn single(key: String, request: InputRequest, state: String) -> Self {
        let mut input_requests = HashMap::with_capacity(1);
        input_requests.insert(key, request);
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

    /// The union must keep the flat `{ method, params }` envelope for every
    /// kind — the `method` is the discriminator, not a nested tag.
    #[test]
    fn every_input_kind_roundtrips_as_a_method_params_envelope() {
        #[allow(deprecated)]
        let cases = [
            (
                InputRequest::Elicitation(ElicitRequestParams::form("Your name?").into()),
                "elicitation/create",
            ),
            (
                InputRequest::Sampling(Box::default()),
                "sampling/createMessage",
            ),
            (InputRequest::Roots(Box::default()), "roots/list"),
        ];

        for (request, method) in cases {
            assert_eq!(request.method(), method);

            let json = serde_json::to_value(&request).unwrap();
            assert_eq!(json["method"], serde_json::json!(method));
            assert!(
                json.get("params").is_some(),
                "the envelope must carry `params` for {method}: {json}"
            );

            let back: InputRequest = serde_json::from_value(json).unwrap();
            assert_eq!(back.method(), method, "kind must survive the round trip");
        }
    }

    /// A peer's `roots/list` envelope may omit `params` entirely; the
    /// params type is all-optional, so it must still decode.
    #[test]
    fn a_roots_envelope_decodes_without_params() {
        let json = serde_json::json!({ "method": "roots/list", "params": {} });
        let parsed: InputRequest = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.method(), "roots/list");
    }

    // `allows` is the server's gate, so it only exists in server builds.
    #[cfg(feature = "server")]
    #[test]
    fn capabilities_gate_each_kind_independently() {
        #[allow(deprecated)]
        let sampling = InputRequest::Sampling(Box::default());
        let elicitation = InputRequest::Elicitation(ElicitRequestParams::form("m").into());

        let only_elicitation = ClientMrtrCapabilities {
            elicitation: true,
            ..Default::default()
        };
        assert!(only_elicitation.allows(&elicitation));
        assert!(
            !only_elicitation.allows(&sampling),
            "a client that only does elicitation must not be asked to sample"
        );

        let all = ClientMrtrCapabilities {
            elicitation: true,
            sampling: true,
            roots: true,
        };
        assert!(all.allows(&sampling));
    }

    /// The flags are additive on the wire: a peer that predates
    /// sampling/roots sends only `elicitation`, and the absent flags must
    /// decode as "not supported" rather than failing.
    #[test]
    fn capabilities_decode_from_an_older_peer() {
        let caps: ClientMrtrCapabilities =
            serde_json::from_value(serde_json::json!({ "elicitation": true })).unwrap();
        assert!(caps.elicitation);
        assert!(!caps.sampling);
        assert!(!caps.roots);

        // …and a default set serializes to nothing at all.
        let json = serde_json::to_value(ClientMrtrCapabilities::default()).unwrap();
        assert_eq!(json, serde_json::json!({}));
    }
}

//! Multi Round-Trip Request (MRTR) wire types (MCP 2026-07-28).
//!
//! A server processing `tools/call` / `prompts/get` / `resources/read` may
//! reply with [`InputRequiredResult`] to request additional input before
//! completing. The kind of input is the [`InputRequest`] union: elicitation
//! (first-class), plus the deprecated-on-arrival sampling and roots kinds the
//! spec re-homed here when it removed their capability-driven server->client
//! requests.
//!
//! # `requestState`: sealed, not merely signed
//!
//! MRTR is stateless -- all cross-round progress travels through the client, which
//! echoes the opaque `requestState` blob back on each retry. How that blob is
//! protected is therefore a design decision, not a detail. neva **seals** it with
//! ChaCha20-Poly1305 (AEAD) rather than **signing** it (HMAC).
//!
//! A signed state is tamper-*evident*: the client cannot alter it undetected, but
//! it can **read** it. That suffices while the state carries only what the client
//! already knows -- the answers it supplied itself. It stops sufficing the moment
//! the server puts its *own* data in there, which is precisely what
//! `Context::memo` does: a memoized value is
//! server-computed -- an upstream API response, a quoted price, a record looked up
//! under the caller's identity, a downstream token -- and it is written into the
//! state so the next round replays it instead of recomputing it. Signing alone
//! would publish every such value to the client, and to anything that logs a
//! request body in between.
//!
//! Nothing is traded away for that confidentiality: the AEAD tag authenticates the
//! payload exactly as an HMAC would, and the `v1.{kid}` header is bound in as
//! associated data so no segment can be transplanted between blobs. The payload
//! additionally carries a TTL, a binding to the originating request, and a binding
//! to the authenticated principal.
//!
//! Two practical consequences:
//! * `ctx.memo` is safe to use for values the client must not see.
//! * The shared secret set via
//!   `App::with_request_state_secret`
//!   (rotated via
//!   `App::with_request_state_keys`)
//!   upholds confidentiality, not just integrity -- treat it as a secret.
//!
//! # Side effects across rounds are the framework's problem
//!
//! Re-run + replay means a handler executes from the top on *every* round, so
//! anything with a side effect between rounds is at risk of running more than
//! once. The protocol itself says nothing about this -- it is left to each
//! implementation, and an SDK may reasonably hand the problem to the application
//! author. neva does not:
//!
//! * `Context::memo` -- compute once, replay the value on
//!   later rounds (sealed into `requestState`, hence the section above).
//! * `Context::once` -- run an effect at most once across
//!   the whole chain.
//! * `Context::on_commit` -- defer an effect until the
//!   handler actually reaches its final result, so an abandoned or failed chain
//!   never applies it.
//! * `RequestStateStore` -- close the one gap the
//!   sealed state structurally cannot: the final round mints no new state, so a
//!   lost HTTP response would otherwise re-run the handler and its commits on
//!   retry. The store caches the committed final response and replays it verbatim.
//!   It is on by default (in-process); a multi-instance deployment supplies a
//!   shared implementation.
//!
//! Together these make an MRTR handler safe to write in the obvious way -- charge
//! the card, send the receipt -- without hand-rolling idempotency keys per tool.
//! See `docs/specs/2026-05-30-mrtr-design.md` for the full design.

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

    /// Server-assigned-key -> elicitation request the client must fulfil.
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

/// Map of server-assigned key -> the input request envelope the client must
/// fulfil.
pub type InputRequests = HashMap<String, InputRequest>;

/// Map of key (matching an [`InputRequests`] key) -> the client's raw result.
///
/// The value stays a [`serde_json::Value`] because the result *type* depends on
/// the kind of input that was requested ([`ElicitResult`](crate::types::elicitation::ElicitResult),
/// [`CreateMessageResult`](crate::types::sampling::CreateMessageResult),
/// [`ListRootsResult`](crate::types::root::ListRootsResult)). Each server-side
/// helper deserializes its own type out of the replay log, exactly like
/// `ctx.memo` does.
pub type InputResponses = HashMap<String, serde_json::Value>;

/// One `{ method, params }` input-request envelope -- the kind of input the
/// server is asking the client for.
///
/// The spec did not delete sampling and roots when the capability-driven
/// server->client requests went away: it re-homed them here, as MRTR input
/// request kinds, keyed by `method`. Elicitation stays first-class; the other
/// two arrive **already deprecated** (see the variant docs), matching the
/// spec's own 12-month lifecycle for roots/sampling/logging.
///
/// Intentionally *not* [`crate::types::Request`]: the wire shape is exactly
/// `{ method, params }` (the per-key id is the map key), whereas `Request`
/// has required `jsonrpc`/`id` fields -- emitting it would add non-spec fields
/// and deserializing a conformant peer's bare `{method,params}` would fail.
/// Deserialization is hand-written rather than derived (see the `impl` below):
/// the adjacent tag would make `params` mandatory, and a conforming peer may
/// omit it for a kind that needs none.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "method", content = "params")]
pub enum InputRequest {
    /// `elicitation/create` -- ask the end user for structured input.
    #[serde(rename = "elicitation/create")]
    Elicitation(ElicitRequestParams),

    /// `sampling/createMessage` -- ask the client's LLM for a completion.
    ///
    /// **Deprecated on arrival.** The ability returns re-homed onto MRTR, but
    /// it stays on the spec's deprecation path; prefer designing tools that do
    /// not need the client's model.
    #[serde(rename = "sampling/createMessage")]
    #[deprecated(
        note = "sampling is deprecated in MCP 2026-07-28; it returns as an MRTR input-request kind only for migration"
    )]
    Sampling(Box<CreateMessageRequestParams>),

    /// `roots/list` -- ask the client which filesystem roots it exposes.
    ///
    /// **Deprecated on arrival**, same as [`Self::Sampling`].
    #[serde(rename = "roots/list")]
    #[deprecated(
        note = "roots are deprecated in MCP 2026-07-28; they return as an MRTR input-request kind only for migration"
    )]
    Roots(Box<ListRootsRequestParams>),
}

impl<'de> Deserialize<'de> for InputRequest {
    /// Decodes a `{ method, params }` envelope, dispatching on `method`.
    ///
    /// `params` is optional on the wire: `roots/list` takes none, so a
    /// conforming peer may send a bare `{"method": "roots/list"}` (or an
    /// explicit `null`). An absent value is read as an empty object rather
    /// than as "use the default", so each kind's own params type still decides
    /// what is acceptable -- `roots/list` decodes (every field is optional)
    /// while a paramless `elicitation/create` or `sampling/createMessage`
    /// fails with that type's own error, as it should.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use crate::types::{elicitation, root, sampling};
        use serde::de::Error as DeError;

        #[derive(Deserialize)]
        struct Envelope {
            method: String,
            #[serde(default)]
            params: Option<serde_json::Value>,
        }

        let envelope = Envelope::deserialize(deserializer)?;
        let params = envelope
            .params
            .filter(|params| !params.is_null())
            .unwrap_or_else(|| serde_json::Value::Object(Default::default()));

        fn parse<T: serde::de::DeserializeOwned, E: DeError>(
            value: serde_json::Value,
        ) -> Result<T, E> {
            serde_json::from_value(value).map_err(E::custom)
        }

        #[allow(deprecated)]
        match envelope.method.as_str() {
            elicitation::commands::CREATE => parse(params).map(Self::Elicitation),
            sampling::commands::CREATE => parse(params).map(Self::Sampling),
            root::commands::LIST => parse(params).map(Self::Roots),
            unknown => Err(D::Error::custom(format!(
                "unknown MRTR input request method `{unknown}`"
            ))),
        }
    }
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
///
/// # Wire shape
///
/// This is the `io.modelcontextprotocol/clientCapabilities` value of a request's
/// `_meta`, which the spec types as `ClientCapabilities`: each capability is an
/// **optional object** whose mere presence declares support (`elicitation` and
/// `sampling` may carry sub-capabilities, `roots` is empty). The flags here are
/// therefore serialized as empty objects and deserialized from any object; a
/// bare boolean is also accepted on the way in, since earlier neva clients wrote
/// one.
///
/// # Examples
/// ```
/// use neva::types::mrtr::ClientMrtrCapabilities;
///
/// // Spec shape: presence of the object is the declaration.
/// let caps: ClientMrtrCapabilities = serde_json::from_value(serde_json::json!({
///     "elicitation": { "form": {} },
///     "roots": {}
/// }))?;
/// assert!(caps.elicitation);
/// assert!(caps.roots);
/// assert!(!caps.sampling);
///
/// assert_eq!(
///     serde_json::to_value(caps)?,
///     serde_json::json!({ "elicitation": {}, "roots": {} })
/// );
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ClientMrtrCapabilities {
    /// Whether the client can fulfil `elicitation/create` input requests.
    #[serde(
        default,
        deserialize_with = "de_declared",
        serialize_with = "ser_declared",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub elicitation: bool,

    /// Whether the client can fulfil `sampling/createMessage` input requests.
    ///
    /// A **deprecated** request kind -- see [`InputRequest::Sampling`].
    #[serde(
        default,
        deserialize_with = "de_declared",
        serialize_with = "ser_declared",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub sampling: bool,

    /// Whether the client can fulfil `roots/list` input requests.
    ///
    /// A **deprecated** request kind -- see [`InputRequest::Roots`].
    #[serde(
        default,
        deserialize_with = "de_declared",
        serialize_with = "ser_declared",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub roots: bool,
}

/// How a single capability may be spelled inside
/// `io.modelcontextprotocol/clientCapabilities`.
#[derive(Deserialize)]
#[serde(untagged)]
enum Declaration {
    /// The spec shape: an object, possibly carrying sub-capabilities. Present
    /// means supported, whatever it contains.
    Object(AnyObject),
    /// What neva's own client wrote before it followed the spec shape.
    Flag(bool),
}

/// Any JSON object, whatever it holds -- the spec declares a capability by the
/// presence of its object, not by anything inside it. Sub-capabilities are
/// accepted and ignored (serde skips unknown fields of a fieldless struct).
#[derive(Deserialize)]
struct AnyObject {}

/// Reads a capability that the spec spells as an optional object, tolerating the
/// boolean older neva clients sent.
fn de_declared<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(match Option::<Declaration>::deserialize(deserializer)? {
        Some(Declaration::Object(_)) => true,
        Some(Declaration::Flag(flag)) => flag,
        None => false,
    })
}

/// Writes a declared capability in the spec shape: an empty object. Only ever
/// called for a `true` flag -- a `false` one is skipped, which is how the spec
/// spells "not supported".
fn ser_declared<S>(_declared: &bool, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeMap;
    serializer.serialize_map(Some(0))?.end()
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

    /// The capability set the server needs for `request`, as the
    /// `requiredCapabilities` payload of a
    /// [`MissingRequiredClientCapability`](crate::error::ErrorCode::MissingRequiredClientCapability)
    /// error -- so the client is told what to declare, not just that something
    /// was missing.
    pub(crate) fn requiring(&self, request: &InputRequest) -> Self {
        #[allow(deprecated)]
        Self {
            elicitation: matches!(request, InputRequest::Elicitation(_)),
            sampling: matches!(request, InputRequest::Sampling(_)),
            roots: matches!(request, InputRequest::Roots(_)),
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
    fn client_capabilities_read_the_spec_object_shape() {
        // What a spec-conformant client (e.g. MCP Inspector) puts in `_meta`:
        // every capability is an object, sub-capabilities and all.
        let caps: ClientMrtrCapabilities = serde_json::from_value(serde_json::json!({
            "elicitation": { "form": {}, "url": {} },
            "sampling": { "context": {}, "tools": {} },
            "roots": {}
        }))
        .expect("object-shaped capabilities must parse");

        assert!(caps.elicitation);
        assert!(caps.sampling);
        assert!(caps.roots);
    }

    #[test]
    fn client_capabilities_read_the_legacy_boolean_shape() {
        let caps: ClientMrtrCapabilities = serde_json::from_value(serde_json::json!({
            "elicitation": true,
            "sampling": false
        }))
        .expect("boolean-shaped capabilities must still parse");

        assert!(caps.elicitation);
        assert!(!caps.sampling);
        assert!(!caps.roots);
    }

    #[test]
    fn absent_and_null_client_capabilities_declare_nothing() {
        let empty: ClientMrtrCapabilities =
            serde_json::from_value(serde_json::json!({})).expect("empty object must parse");
        assert!(!empty.elicitation && !empty.sampling && !empty.roots);

        let nulls: ClientMrtrCapabilities =
            serde_json::from_value(serde_json::json!({ "elicitation": null, "roots": null }))
                .expect("null capabilities must parse");
        assert!(!nulls.elicitation && !nulls.roots);
    }

    #[test]
    fn client_capabilities_write_the_spec_object_shape() {
        let caps = ClientMrtrCapabilities {
            elicitation: true,
            sampling: false,
            roots: true,
        };

        assert_eq!(
            serde_json::to_value(caps).expect("serialize"),
            serde_json::json!({ "elicitation": {}, "roots": {} })
        );
    }

    #[test]
    fn client_capabilities_roundtrip() {
        let caps = ClientMrtrCapabilities {
            elicitation: true,
            sampling: true,
            roots: false,
        };
        let back: ClientMrtrCapabilities =
            serde_json::from_value(serde_json::to_value(caps).expect("serialize"))
                .expect("deserialize");

        assert!(back.elicitation);
        assert!(back.sampling);
        assert!(!back.roots);
    }

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
    /// kind -- the `method` is the discriminator, not a nested tag.
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
    fn a_roots_envelope_decodes_with_or_without_params() {
        for json in [
            serde_json::json!({ "method": "roots/list", "params": {} }),
            // A conforming peer may omit the empty object entirely.
            serde_json::json!({ "method": "roots/list" }),
            serde_json::json!({ "method": "roots/list", "params": null }),
        ] {
            let parsed: InputRequest = serde_json::from_value(json.clone())
                .unwrap_or_else(|err| panic!("{json} must decode: {err}"));
            assert_eq!(parsed.method(), "roots/list");
        }
    }

    /// Absent params is *not* a blanket "use the default": a kind whose params
    /// carry required fields still fails, with its own error.
    #[test]
    fn a_paramless_envelope_still_fails_for_kinds_that_need_params() {
        for method in ["elicitation/create", "sampling/createMessage"] {
            let json = serde_json::json!({ "method": method });
            assert!(
                serde_json::from_value::<InputRequest>(json).is_err(),
                "{method} must not decode without params"
            );
        }
    }

    #[test]
    fn an_unknown_input_kind_is_rejected_by_name() {
        let json = serde_json::json!({ "method": "sorcery/summon", "params": {} });
        let err = serde_json::from_value::<InputRequest>(json).unwrap_err();
        assert!(
            err.to_string().contains("sorcery/summon"),
            "the error must name the unknown method, got: {err}"
        );
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

        // ...and a default set serializes to nothing at all.
        let json = serde_json::to_value(ClientMrtrCapabilities::default()).unwrap();
        assert_eq!(json, serde_json::json!({}));
    }
}

//! Encode/verify the opaque, HMAC-protected `requestState` blob.

use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, ErrorCode};
use crate::types::mrtr::InputResponses;

type HmacSha256 = Hmac<Sha256>;

/// The signed contents of a `requestState` blob.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StatePayload {
    /// Monotonically-growing replay log of answered inputs.
    pub answers: InputResponses,
    /// Keys the server requested in the round that minted this state. The next
    /// round's `inputResponses` may only answer these keys (and only ones not
    /// already present in [`StatePayload::answers`]); anything else is unsolicited and
    /// must be rejected, otherwise a client could pre-seed/overwrite answers
    /// for inputs the server never asked for. Defaults to empty so legacy blobs
    /// decode — and, by design, reject any `inputResponses` paired with them.
    #[serde(default)]
    pub requested: Vec<String>,
    /// Cached `ctx.memo` values, keyed by memo key.
    #[serde(default)]
    pub memos: std::collections::HashMap<String, serde_json::Value>,
    /// Executed `ctx.once` effect keys.
    #[serde(default)]
    pub effects: std::collections::HashSet<String>,
    /// Unix-seconds expiry.
    pub exp: u64,
    /// Request binding: `"{method}:{hex(sha256(salient_params))}"`.
    pub req: String,
    /// Authenticated principal (subject), when auth is enabled.
    pub principal: Option<String>,
}

/// Encodes/decodes [`StatePayload`] as `b64(payload).b64(hmac)`.
pub(crate) struct StateCodec<'a> {
    key: &'a [u8],
}

impl<'a> StateCodec<'a> {
    /// Creates a codec bound to a signing key.
    pub(crate) fn new(key: &'a [u8]) -> Self {
        Self { key }
    }

    fn mac(&self, bytes: &[u8]) -> Result<Vec<u8>, Error> {
        let mut mac = HmacSha256::new_from_slice(self.key)
            .map_err(|_| Error::new(ErrorCode::InternalError, "bad hmac key"))?;
        mac.update(bytes);
        Ok(mac.finalize().into_bytes().to_vec())
    }

    /// Encodes a payload into the opaque wire string.
    pub(crate) fn encode(&self, payload: &StatePayload) -> Result<String, Error> {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
        let json = serde_json::to_vec(payload).map_err(Error::from)?;
        let tag = self.mac(&json)?;
        Ok(format!("{}.{}", B64.encode(&json), B64.encode(&tag)))
    }

    /// Decodes and verifies integrity. Does NOT check `exp`/`req`/`principal` —
    /// callers do that against the returned [`StatePayload`].
    pub(crate) fn decode(&self, blob: &str) -> Result<StatePayload, Error> {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
        let (p_b64, t_b64) = blob
            .split_once('.')
            .ok_or_else(|| Error::new(ErrorCode::InvalidParams, "malformed requestState"))?;
        let json = B64
            .decode(p_b64)
            .map_err(|_| Error::new(ErrorCode::InvalidParams, "bad requestState payload"))?;
        let tag = B64
            .decode(t_b64)
            .map_err(|_| Error::new(ErrorCode::InvalidParams, "bad requestState tag"))?;
        let mut mac = HmacSha256::new_from_slice(self.key)
            .map_err(|_| Error::new(ErrorCode::InternalError, "bad hmac key"))?;
        mac.update(&json);
        mac.verify_slice(&tag).map_err(|_| {
            Error::new(
                ErrorCode::InvalidParams,
                "requestState integrity check failed",
            )
        })?;
        serde_json::from_slice(&json).map_err(Error::from)
    }
}

/// Current unix-seconds (saturating to 0 before the epoch).
pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Builds the request-binding string `"{method}:{b64(sha256(params))}"`.
pub(crate) fn request_binding(method: &str, salient_params: &serde_json::Value) -> String {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
    use sha2::Digest;
    // Canonicalize object keys before hashing. `serde_json::to_vec` follows the
    // map's iteration order, which is lexicographic for the default `BTreeMap`
    // backing but *insertion* order when any dependency in the build enables
    // serde_json's `preserve_order` feature. Without canonicalization an MRTR
    // retry carrying semantically identical params with a different key order
    // would hash differently and be rejected as not matching the request.
    let bytes = serde_json::to_vec(&canonicalize(salient_params)).unwrap_or_default();
    let digest = Sha256::digest(&bytes);
    format!("{method}:{}", B64.encode(digest))
}

/// Stable digest of a round's `inputResponses`, used as part of the MRTR
/// final-response cache key (`b64(sha256(canonical(responses)))`).
///
/// Two concurrent flows can reach the same pre-answer `requestState` — the
/// payload carries no nonce, so identical method/params/principal minted in the
/// same second produce the same integrity tag — yet supply different answers.
/// Folding this digest into the cache key keeps those flows from colliding,
/// while a genuine lost-response retry (same state *and* same answers) still
/// hits. Object keys are canonicalized so the digest is independent of map
/// iteration order.
pub(crate) fn input_responses_digest(responses: &InputResponses) -> String {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
    use sha2::Digest;
    let value = serde_json::to_value(responses).unwrap_or_default();
    let bytes = serde_json::to_vec(&canonicalize(&value)).unwrap_or_default();
    B64.encode(Sha256::digest(&bytes))
}

/// Returns a copy of `value` with every object's keys ordered lexicographically,
/// recursively, so its serialization is stable regardless of serde_json's
/// `preserve_order` feature. Arrays keep their order (significant in JSON).
fn canonicalize(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            let mut out = serde_json::Map::with_capacity(keys.len());
            for key in keys {
                out.insert(key.clone(), canonicalize(&map[key]));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(canonicalize).collect())
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn payload() -> StatePayload {
        StatePayload {
            answers: HashMap::new(),
            requested: Vec::new(),
            memos: HashMap::new(),
            effects: std::collections::HashSet::new(),
            exp: now_secs() + 300,
            req: request_binding("tools/call", &serde_json::json!({"name":"t"})),
            principal: Some("alice".into()),
        }
    }

    #[test]
    fn memos_and_effects_roundtrip() {
        let codec = StateCodec::new(b"secret-key");
        let mut p = payload();
        p.memos
            .insert("quote".into(), serde_json::json!({"price": 42}));
        p.effects.insert("charge".into());
        let blob = codec.encode(&p).unwrap();
        let got = codec.decode(&blob).unwrap();
        assert_eq!(
            got.memos.get("quote"),
            Some(&serde_json::json!({"price": 42}))
        );
        assert!(got.effects.contains("charge"));
    }

    #[test]
    fn old_blob_without_memos_or_effects_still_decodes() {
        // A payload serialized before memos/effects existed: omit both keys.
        let json = serde_json::json!({
            "answers": {},
            "exp": now_secs() + 300,
            "req": request_binding("tools/call", &serde_json::json!({"name":"t"})),
            "principal": serde_json::Value::Null,
        });
        let codec = StateCodec::new(b"secret-key");
        let bytes = serde_json::to_vec(&json).unwrap();
        // Re-sign so the HMAC matches the legacy bytes. `Hmac`, `Sha256`, `Mac`
        // and `KeyInit` are already in scope via `use super::*`.
        let blob = {
            use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
            let mut mac = <Hmac<Sha256>>::new_from_slice(b"secret-key").unwrap();
            mac.update(&bytes);
            format!(
                "{}.{}",
                B64.encode(&bytes),
                B64.encode(mac.finalize().into_bytes())
            )
        };
        let got = codec.decode(&blob).unwrap();
        assert!(got.memos.is_empty());
        assert!(got.effects.is_empty());
    }

    #[test]
    fn encode_decode_roundtrips() {
        let codec = StateCodec::new(b"secret-key");
        let p = payload();
        let blob = codec.encode(&p).unwrap();
        let got = codec.decode(&blob).unwrap();
        assert_eq!(got.exp, p.exp);
        assert_eq!(got.req, p.req);
        assert_eq!(got.principal, p.principal);
        assert!(got.answers.is_empty());
    }

    #[test]
    fn tampered_blob_is_rejected() {
        let codec = StateCodec::new(b"secret-key");
        let mut blob = codec.encode(&payload()).unwrap();
        blob.push('x'); // corrupt the tag
        assert!(codec.decode(&blob).is_err());
    }

    #[test]
    fn wrong_key_is_rejected() {
        let blob = StateCodec::new(b"key-a").encode(&payload()).unwrap();
        assert!(StateCodec::new(b"key-b").decode(&blob).is_err());
    }

    #[test]
    fn request_binding_is_stable_and_distinct() {
        let a = request_binding("tools/call", &serde_json::json!({"name":"t"}));
        let b = request_binding("tools/call", &serde_json::json!({"name":"t"}));
        let c = request_binding("tools/call", &serde_json::json!({"name":"u"}));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn request_binding_is_independent_of_object_key_order() {
        // Two semantically identical params differing only in key order, nested
        // inside an object and an array. With serde_json's `preserve_order`
        // feature these would serialize differently; canonicalization makes the
        // binding stable so an MRTR retry is not spuriously rejected.
        let mut first = serde_json::Map::new();
        first.insert("name".into(), serde_json::json!("t"));
        first.insert(
            "args".into(),
            serde_json::json!([{"a": 1, "b": 2}, {"c": 3}]),
        );

        let mut second = serde_json::Map::new();
        second.insert(
            "args".into(),
            serde_json::json!([{"b": 2, "a": 1}, {"c": 3}]),
        );
        second.insert("name".into(), serde_json::json!("t"));

        assert_eq!(
            request_binding("tools/call", &serde_json::Value::Object(first)),
            request_binding("tools/call", &serde_json::Value::Object(second)),
        );
    }

    fn answer(content: serde_json::Value) -> crate::types::elicitation::ElicitResult {
        crate::types::elicitation::ElicitResult {
            action: crate::types::elicitation::ElicitationAction::Accept,
            content: Some(content),
            meta: None,
        }
    }

    #[test]
    fn input_responses_digest_distinguishes_distinct_answers() {
        let mut a = InputResponses::new();
        a.insert("k".into(), answer(serde_json::json!({"v": 1})));
        let mut b = InputResponses::new();
        b.insert("k".into(), answer(serde_json::json!({"v": 2})));

        assert_ne!(input_responses_digest(&a), input_responses_digest(&b));
        // Same answers digest the same (stable across constructions).
        let mut a2 = InputResponses::new();
        a2.insert("k".into(), answer(serde_json::json!({"v": 1})));
        assert_eq!(input_responses_digest(&a), input_responses_digest(&a2));
    }

    #[test]
    fn input_responses_digest_is_independent_of_key_order() {
        let mut first = InputResponses::new();
        first.insert("a".into(), answer(serde_json::json!({"x": 1, "y": 2})));
        first.insert("b".into(), answer(serde_json::json!(null)));

        let mut second = InputResponses::new();
        second.insert("b".into(), answer(serde_json::json!(null)));
        second.insert("a".into(), answer(serde_json::json!({"y": 2, "x": 1})));

        assert_eq!(
            input_responses_digest(&first),
            input_responses_digest(&second)
        );
    }
}

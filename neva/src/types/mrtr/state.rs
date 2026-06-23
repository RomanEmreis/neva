//! Encode/verify the opaque, encrypted `requestState` blob.
//!
//! The blob is sealed with ChaCha20-Poly1305 (AEAD): the AEAD tag provides
//! integrity (replacing the former HMAC) *and* the payload is encrypted, so
//! server-side values a handler caches via [`crate::Context::memo`] — API
//! responses, PII, tokens — are confidential rather than merely signed and
//! readable by the client that echoes the blob.

use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, ErrorCode};
use crate::types::mrtr::InputResponses;

/// ChaCha20-Poly1305 nonce length (96 bits).
const NONCE_LEN: usize = 12;

/// Derives the 32-byte AEAD key from the configured secret (which may be any
/// length). Domain-separated so the key is specific to this use and version.
fn derive_key(secret: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    let mut h = Sha256::new();
    h.update(b"neva:mrtr:requestState:v1");
    h.update(secret);
    h.finalize().into()
}

/// The encrypted contents of a `requestState` blob.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StatePayload {
    /// Monotonically-growing replay log of answered inputs.
    pub answers: InputResponses,
    /// Keys the server requested in the round that minted this state. The next
    /// round's `inputResponses` may only answer these keys (and only ones not
    /// already present in [`StatePayload::answers`]); anything else is unsolicited and
    /// must be rejected, otherwise a client could pre-seed/overwrite answers
    /// for inputs the server never asked for. Defaults to empty so an older
    /// payload schema still decodes — and, by design, rejects any
    /// `inputResponses` paired with it.
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

/// Encodes/decodes [`StatePayload`] as `b64(nonce).b64(ciphertext+tag)`, sealed
/// with ChaCha20-Poly1305. The trailing segment (the ciphertext with its AEAD
/// tag) is unique per minted state and is what the dispatch layer uses as the
/// per-state identity for the idempotency cache.
pub(crate) struct StateCodec<'a> {
    key: &'a [u8],
}

impl<'a> StateCodec<'a> {
    /// Creates a codec bound to an encryption secret.
    pub(crate) fn new(key: &'a [u8]) -> Self {
        Self { key }
    }

    fn cipher(&self) -> Result<ChaCha20Poly1305, Error> {
        ChaCha20Poly1305::new_from_slice(&derive_key(self.key))
            .map_err(|_| Error::new(ErrorCode::InternalError, "bad state key"))
    }

    /// Encrypts a payload into the opaque wire string.
    pub(crate) fn encode(&self, payload: &StatePayload) -> Result<String, Error> {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
        let json = serde_json::to_vec(payload).map_err(Error::from)?;
        let cipher = self.cipher()?;
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let sealed = cipher
            .encrypt(&nonce, json.as_slice())
            .map_err(|_| Error::new(ErrorCode::InternalError, "requestState encryption failed"))?;
        Ok(format!("{}.{}", B64.encode(nonce), B64.encode(sealed)))
    }

    /// Decrypts and verifies integrity. Does NOT check `exp`/`req`/`principal` —
    /// callers do that against the returned [`StatePayload`].
    pub(crate) fn decode(&self, blob: &str) -> Result<StatePayload, Error> {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
        let (n_b64, c_b64) = blob
            .split_once('.')
            .ok_or_else(|| Error::new(ErrorCode::InvalidParams, "malformed requestState"))?;
        let nonce = B64
            .decode(n_b64)
            .map_err(|_| Error::new(ErrorCode::InvalidParams, "bad requestState nonce"))?;
        let sealed = B64
            .decode(c_b64)
            .map_err(|_| Error::new(ErrorCode::InvalidParams, "bad requestState payload"))?;
        let nonce: [u8; NONCE_LEN] = nonce
            .try_into()
            .map_err(|_| Error::new(ErrorCode::InvalidParams, "bad requestState nonce"))?;
        let json = self
            .cipher()?
            .decrypt(&Nonce::from(nonce), sealed.as_slice())
            .map_err(|_| {
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
/// The same minted `requestState` can be echoed with different answers (a client
/// replaying one round-1 blob with two different `inputResponses`), so the cache
/// key folds in this digest to keep those apart while a genuine lost-response
/// retry (same state *and* same answers) still hits. Object keys are
/// canonicalized so the digest is independent of map iteration order.
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
    fn payload_without_memos_or_effects_decodes_with_defaults() {
        // A payload schema that omits memos/effects/requested (e.g. minted by an
        // older neva): sealed with the codec's own cipher so only the serde
        // `#[serde(default)]` behavior is under test. `memos`/`effects` default
        // to empty and any paired `inputResponses` are rejected by design.
        let json = serde_json::json!({
            "answers": {},
            "exp": now_secs() + 300,
            "req": request_binding("tools/call", &serde_json::json!({"name":"t"})),
            "principal": serde_json::Value::Null,
        });
        let codec = StateCodec::new(b"secret-key");
        let blob = {
            use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
            let bytes = serde_json::to_vec(&json).unwrap();
            let cipher = codec.cipher().unwrap();
            let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
            let sealed = cipher.encrypt(&nonce, bytes.as_slice()).unwrap();
            format!("{}.{}", B64.encode(nonce), B64.encode(sealed))
        };
        let got = codec.decode(&blob).unwrap();
        assert!(got.memos.is_empty());
        assert!(got.effects.is_empty());
        assert!(got.requested.is_empty());
    }

    #[test]
    fn memo_values_are_not_readable_from_the_wire_blob() {
        // Confidentiality: a secret cached via `ctx.memo` must not be recoverable
        // by decoding the opaque blob without the key.
        let codec = StateCodec::new(b"secret-key");
        let mut p = payload();
        p.memos
            .insert("token".into(), serde_json::json!("super-secret-value"));
        let blob = codec.encode(&p).unwrap();

        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
        for segment in blob.split('.') {
            let bytes = B64.decode(segment).unwrap_or_default();
            assert!(
                !bytes
                    .windows(b"super-secret-value".len())
                    .any(|w| w == b"super-secret-value"),
                "memo value leaked in plaintext within the blob"
            );
        }
        // The holder of the key still recovers it.
        assert_eq!(
            codec.decode(&blob).unwrap().memos.get("token"),
            Some(&serde_json::json!("super-secret-value"))
        );
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

//! Encode/verify the opaque, encrypted `requestState` blob.
//!
//! The blob is sealed with ChaCha20-Poly1305 (AEAD) rather than signed; the
//! rationale is user-facing and lives in the [`mrtr`](crate::types::mrtr)
//! module docs. Beyond confidentiality and the authentication tag, the payload
//! carries a TTL, a binding to the originating request and a binding to the
//! authenticated principal — see [`StatePayload`].

use chacha20poly1305::aead::{Aead, Generate, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, ErrorCode};
use crate::types::mrtr::InputResponses;

/// ChaCha20-Poly1305 nonce length (96 bits).
const NONCE_LEN: usize = 12;

/// Codec version written as the blob's first wire segment and bound into the
/// AEAD associated data, so a blob minted under one format cannot be
/// transplanted into a future one. Bump when the wire format or the
/// [`StatePayload`] semantics change; [`StateCodec::decode`] rejects anything else.
pub(crate) const STATE_VERSION: &str = "v1";

/// Key id used when a single secret is configured (the
/// [`crate::App::with_request_state_secret`] path).
pub(crate) const DEFAULT_KID: &str = "0";

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

/// The secrets accepted for `requestState` decryption plus the active one used
/// for encryption. Enables zero-downtime key rotation: stage the new key as
/// accepted on every instance, then flip it to active; states minted under the
/// old kid keep verifying until their TTL lapses.
pub(crate) struct StateKeyring {
    /// Kid new blobs are sealed under. Must resolve in [`Self::keys`], or
    /// [`StateCodec::encode`] fails.
    active_kid: Box<str>,
    /// Accepted `kid → secret` map used to select the decryption key by the
    /// kid segment of the inbound blob.
    keys: HashMap<Box<str>, Arc<[u8]>>,
}

impl std::fmt::Debug for StateKeyring {
    // Manual impl so key material never reaches logs: kids only.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StateKeyring")
            .field("active_kid", &self.active_kid)
            .field("kids", &self.keys.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl StateKeyring {
    /// Single-key ring under [`DEFAULT_KID`] — the
    /// [`crate::App::with_request_state_secret`] path.
    pub(crate) fn single(secret: &[u8]) -> Self {
        Self::new(DEFAULT_KID, [(DEFAULT_KID, secret)])
    }

    /// Builds a ring that encodes with `active_kid` and accepts every
    /// `(kid, secret)` in `keys` for decoding.
    pub(crate) fn new<K, S>(active_kid: &str, keys: impl IntoIterator<Item = (K, S)>) -> Self
    where
        K: AsRef<str>,
        S: AsRef<[u8]>,
    {
        Self {
            active_kid: Box::from(active_kid),
            keys: keys
                .into_iter()
                .map(|(kid, secret)| (Box::from(kid.as_ref()), Arc::from(secret.as_ref())))
                .collect(),
        }
    }

    /// The `(kid, secret)` pair new blobs are sealed under.
    fn active(&self) -> Result<(&str, &[u8]), Error> {
        self.keys
            .get(&*self.active_kid)
            .map(|secret| (&*self.active_kid, &**secret))
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::InternalError,
                    "active requestState kid has no key in the keyring",
                )
            })
    }

    /// The secret accepted under `kid`, if any.
    fn key(&self, kid: &str) -> Option<&[u8]> {
        self.keys.get(kid).map(|secret| &**secret)
    }
}

/// Encodes/decodes [`StatePayload`] as
/// `{version}.{kid}.b64(nonce).b64(ciphertext+tag)`, sealed with
/// ChaCha20-Poly1305. The `{version}.{kid}` header is bound into the AEAD
/// associated data, so neither segment can be swapped on the wire without
/// failing the tag. The trailing segment (the ciphertext with its AEAD tag) is
/// unique per minted state and is what the dispatch layer uses as the
/// per-state identity for the idempotency cache.
pub(crate) struct StateCodec<'a> {
    keyring: &'a StateKeyring,
}

impl<'a> StateCodec<'a> {
    /// Creates a codec bound to a keyring.
    pub(crate) fn new(keyring: &'a StateKeyring) -> Self {
        Self { keyring }
    }

    fn cipher(secret: &[u8]) -> Result<ChaCha20Poly1305, Error> {
        ChaCha20Poly1305::new_from_slice(&derive_key(secret))
            .map_err(|_| Error::new(ErrorCode::InternalError, "bad state key"))
    }

    /// Encrypts a payload into the opaque wire string using the keyring's
    /// active key.
    pub(crate) fn encode(&self, payload: &StatePayload) -> Result<String, Error> {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
        let (kid, secret) = self.keyring.active()?;
        if kid.is_empty() || kid.contains('.') {
            // '.' is the segment separator; a kid containing it would shift the
            // nonce/ciphertext segments and every minted blob would fail decode.
            return Err(Error::new(
                ErrorCode::InternalError,
                "requestState kid must be non-empty and must not contain '.'",
            ));
        }
        let json = serde_json::to_vec(payload).map_err(Error::from)?;
        let cipher = Self::cipher(secret)?;
        let nonce = Nonce::try_generate().map_err(|_| {
            Error::new(
                ErrorCode::InternalError,
                "requestState nonce generation failed",
            )
        })?;
        let header = format!("{STATE_VERSION}.{kid}");
        let sealed = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: &json,
                    aad: header.as_bytes(),
                },
            )
            .map_err(|_| Error::new(ErrorCode::InternalError, "requestState encryption failed"))?;
        Ok(format!(
            "{header}.{}.{}",
            B64.encode(nonce),
            B64.encode(sealed)
        ))
    }

    /// Decrypts and verifies integrity, selecting the key by the blob's kid
    /// segment. Does NOT check `exp`/`req`/`principal` — callers do that
    /// against the returned [`StatePayload`].
    pub(crate) fn decode(&self, blob: &str) -> Result<StatePayload, Error> {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
        // Parse a fixed segment count. A left-to-right `split_once` would take
        // the `v1` prefix as the nonce; anything but exactly four segments is
        // malformed (base64url has no '.', so the count is unambiguous).
        let mut parts = blob.split('.');
        let (Some(version), Some(kid), Some(n_b64), Some(c_b64), None) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            return Err(Error::new(
                ErrorCode::InvalidParams,
                "malformed requestState",
            ));
        };
        if version != STATE_VERSION {
            return Err(Error::new(
                ErrorCode::InvalidParams,
                "unsupported requestState version",
            ));
        }
        let secret = self
            .keyring
            .key(kid)
            .ok_or_else(|| Error::new(ErrorCode::InvalidParams, "unknown requestState key id"))?;
        let nonce = B64
            .decode(n_b64)
            .map_err(|_| Error::new(ErrorCode::InvalidParams, "bad requestState nonce"))?;
        let sealed = B64
            .decode(c_b64)
            .map_err(|_| Error::new(ErrorCode::InvalidParams, "bad requestState payload"))?;
        let nonce: [u8; NONCE_LEN] = nonce
            .try_into()
            .map_err(|_| Error::new(ErrorCode::InvalidParams, "bad requestState nonce"))?;
        let header = format!("{STATE_VERSION}.{kid}");
        let json = Self::cipher(secret)?
            .decrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: sealed.as_slice(),
                    aad: header.as_bytes(),
                },
            )
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

    fn ring(secret: &[u8]) -> StateKeyring {
        StateKeyring::single(secret)
    }

    /// Seals arbitrary JSON the way `StateCodec::encode` would, so tests can
    /// mint blobs with non-[`StatePayload`] contents or a custom kid.
    fn seal_json(secret: &[u8], kid: &str, json: &serde_json::Value) -> String {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
        let bytes = serde_json::to_vec(json).unwrap();
        let cipher = StateCodec::cipher(secret).unwrap();
        let nonce = Nonce::try_generate().unwrap();
        let header = format!("{STATE_VERSION}.{kid}");
        let sealed = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: bytes.as_slice(),
                    aad: header.as_bytes(),
                },
            )
            .unwrap();
        format!("{header}.{}.{}", B64.encode(nonce), B64.encode(sealed))
    }

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
        let ring = ring(b"secret-key");
        let codec = StateCodec::new(&ring);
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
        let ring = ring(b"secret-key");
        let codec = StateCodec::new(&ring);
        let blob = seal_json(b"secret-key", DEFAULT_KID, &json);
        let got = codec.decode(&blob).unwrap();
        assert!(got.memos.is_empty());
        assert!(got.effects.is_empty());
        assert!(got.requested.is_empty());
    }

    #[test]
    fn memo_values_are_not_readable_from_the_wire_blob() {
        // Confidentiality: a secret cached via `ctx.memo` must not be recoverable
        // by decoding the opaque blob without the key.
        let ring = ring(b"secret-key");
        let codec = StateCodec::new(&ring);
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
        let ring = ring(b"secret-key");
        let codec = StateCodec::new(&ring);
        let p = payload();
        let blob = codec.encode(&p).unwrap();
        let got = codec.decode(&blob).unwrap();
        assert_eq!(got.exp, p.exp);
        assert_eq!(got.req, p.req);
        assert_eq!(got.principal, p.principal);
        assert!(got.answers.is_empty());
    }

    #[test]
    fn encode_writes_version_and_kid_segments() {
        let ring = ring(b"secret-key");
        let blob = StateCodec::new(&ring).encode(&payload()).unwrap();
        let segments: Vec<&str> = blob.split('.').collect();
        assert_eq!(segments.len(), 4, "expected v1.kid.nonce.ct, got {blob}");
        assert_eq!(segments[0], STATE_VERSION);
        assert_eq!(segments[1], DEFAULT_KID);
    }

    #[test]
    fn tampered_blob_is_rejected() {
        let ring = ring(b"secret-key");
        let codec = StateCodec::new(&ring);
        let mut blob = codec.encode(&payload()).unwrap();
        blob.push('x'); // corrupt the tag
        assert!(codec.decode(&blob).is_err());
    }

    #[test]
    fn wrong_key_is_rejected() {
        let ring_a = ring(b"key-a");
        let ring_b = ring(b"key-b");
        let blob = StateCodec::new(&ring_a).encode(&payload()).unwrap();
        assert!(StateCodec::new(&ring_b).decode(&blob).is_err());
    }

    #[test]
    fn unknown_version_is_rejected() {
        let ring = ring(b"secret-key");
        let codec = StateCodec::new(&ring);
        let blob = codec.encode(&payload()).unwrap();
        let transplanted = format!("v9{}", blob.strip_prefix("v1").unwrap());
        let err = codec.decode(&transplanted).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
        assert!(
            format!("{err}").contains("unsupported requestState version"),
            "{err}"
        );
    }

    #[test]
    fn unknown_kid_is_rejected() {
        let ring = ring(b"secret-key");
        let codec = StateCodec::new(&ring);
        // A blob sealed under a kid the ring does not accept.
        let blob = seal_json(b"secret-key", "9", &serde_json::json!({}));
        let err = codec.decode(&blob).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
        assert!(
            format!("{err}").contains("unknown requestState key id"),
            "{err}"
        );
    }

    #[test]
    fn legacy_two_segment_blob_is_rejected_as_malformed() {
        // The pre-versioning wire format (`b64(nonce).b64(ct)`) must not be
        // taken for a v1 blob — segment-count parsing rejects it outright.
        let ring = ring(b"secret-key");
        let codec = StateCodec::new(&ring);
        let blob = codec.encode(&payload()).unwrap();
        let legacy = blob.splitn(3, '.').nth(2).unwrap(); // strip "v1.kid."
        let err = codec.decode(legacy).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
        assert!(format!("{err}").contains("malformed requestState"), "{err}");
    }

    #[test]
    fn kid_transplant_fails_the_aad_binding() {
        // Two kids sharing one secret: rewriting the wire kid selects the SAME
        // decryption key, so only the AAD binding of `{version}.{kid}` can
        // catch the transplant.
        let ring = StateKeyring::new("a", [("a", b"same-secret"), ("b", b"same-secret")]);
        let codec = StateCodec::new(&ring);
        let blob = codec.encode(&payload()).unwrap();
        let transplanted = format!("v1.b{}", blob.strip_prefix("v1.a").unwrap());
        let err = codec.decode(&transplanted).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
        assert!(format!("{err}").contains("integrity check failed"), "{err}");
        // Sanity: the untouched blob still decodes.
        assert!(codec.decode(&blob).is_ok());
    }

    #[test]
    fn rotated_keyring_still_decodes_old_kid() {
        // Rotation: blobs minted while "1" was active keep verifying after the
        // ring flips to "2", as long as "1" stays accepted.
        let old = StateKeyring::new("1", [("1", b"old-secret")]);
        let blob = StateCodec::new(&old).encode(&payload()).unwrap();

        let rotated = StateKeyring::new(
            "2",
            [
                ("2", b"new-secret".as_slice()),
                ("1", b"old-secret".as_slice()),
            ],
        );
        let codec = StateCodec::new(&rotated);
        assert!(codec.decode(&blob).is_ok());
        // New blobs are sealed under the new active kid.
        let fresh = codec.encode(&payload()).unwrap();
        assert!(fresh.starts_with("v1.2."), "{fresh}");
        // Dropping the old key from the ring retires its blobs.
        let retired = StateKeyring::new("2", [("2", b"new-secret")]);
        assert!(StateCodec::new(&retired).decode(&blob).is_err());
    }

    #[test]
    fn active_kid_missing_from_ring_fails_encode() {
        let ring = StateKeyring::new("active", [("other", b"secret")]);
        let err = StateCodec::new(&ring).encode(&payload()).unwrap_err();
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn kid_with_separator_fails_encode() {
        // '.' inside a kid would shift the wire segments; encode refuses to
        // mint an undecodable blob.
        let ring = StateKeyring::new("a.b", [("a.b", b"secret")]);
        let err = StateCodec::new(&ring).encode(&payload()).unwrap_err();
        assert_eq!(err.code, ErrorCode::InternalError);
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

    /// An answer as it is stored in the replay log: raw JSON, since the result
    /// type depends on which input kind was requested.
    fn answer(content: serde_json::Value) -> serde_json::Value {
        serde_json::to_value(crate::types::elicitation::ElicitResult {
            action: crate::types::elicitation::ElicitationAction::Accept,
            content: Some(content),
            meta: None,
        })
        .expect("an ElicitResult always serializes")
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

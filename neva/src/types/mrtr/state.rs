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
    let bytes = serde_json::to_vec(salient_params).unwrap_or_default();
    let digest = Sha256::digest(&bytes);
    format!("{method}:{}", B64.encode(digest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn payload() -> StatePayload {
        StatePayload {
            answers: HashMap::new(),
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
}

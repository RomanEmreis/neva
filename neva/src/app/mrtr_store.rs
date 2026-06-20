//! Idempotency store for MRTR final-round replay protection
//! (`proto-2026-07-28-rc`).
//!
//! The stateless MRTR transport (see [`crate::types::mrtr`]) signs all
//! cross-round progress into the `requestState` blob the client echoes on each
//! retry, so [`Context::once`](crate::Context::once) /
//! [`Context::memo`](crate::Context::memo) dedup steps *within* a chain without
//! any server-side state. There is one gap that the signed state structurally
//! cannot close: the **final** round mints no new state, so if its HTTP
//! response is lost and the client retries the same `requestState` +
//! `inputResponses`, the handler — and any
//! [`Context::on_commit`](crate::Context::on_commit) commits or post-elicit
//! `once`/`memo` effects — would run a second time.
//!
//! This module's [`RequestStateStore`] closes that gap by caching the final
//! response of a committed round, keyed by the incoming state's sealed segment
//! plus a digest of that round's answers. On a lost-response retry the client
//! echoes the same blob and answers (same key), so the cached response is
//! returned verbatim and the handler never re-executes.
//!
//! The default [`InMemoryStateStore`] is per-process. A multi-instance
//! deployment should supply a shared implementation (e.g. Redis) via
//! [`App::with_request_state_store`](crate::App::with_request_state_store) — for
//! the same reason such a deployment must share the MRTR secret (a retry routed
//! to a different instance must see the same committed state).

use crate::types::Response;
use crate::types::mrtr::state::now_secs;
use futures_util::future::BoxFuture;

/// A store that remembers the final response of a committed MRTR `requestState`
/// so a lost-response retry returns the cached result instead of re-running the
/// handler (`proto-2026-07-28-rc`).
///
/// Implement this to back MRTR idempotency with a shared store across a
/// multi-instance deployment; the entry key is the `requestState` sealed segment
/// (the base64 ciphertext after the `.`) combined with a digest of the round's
/// answers, unique per minted state and round.
///
/// See the [module docs](self) for the full rationale.
pub trait RequestStateStore: Send + Sync {
    /// Returns the cached final response previously recorded for `tag`, or
    /// `None` if that state has not committed yet (or its entry has expired).
    fn get<'a>(&'a self, tag: &'a str) -> BoxFuture<'a, Option<Response>>;

    /// Records `response` as the committed result for `tag`, retained at least
    /// until the unix-seconds `exp` (the originating state's own expiry, after
    /// which a retry is rejected as expired before reaching the store).
    fn put<'a>(&'a self, tag: &'a str, response: Response, exp: u64) -> BoxFuture<'a, ()>;
}

/// The default per-process [`RequestStateStore`], backed by a concurrent map
/// with lazy TTL eviction (`proto-2026-07-28-rc`).
///
/// Entries are bounded by their TTL (the state's `exp`, ≤ the configured
/// `requestState` TTL, 300s by default) and evicted opportunistically on access,
/// so the map's footprint tracks the number of in-flight MRTR flows. Adequate
/// for single-instance and development; multi-instance deployments should
/// provide a shared store (see the [module docs](self)).
#[derive(Debug, Default)]
pub struct InMemoryStateStore {
    entries: dashmap::DashMap<String, (Response, u64)>,
}

impl InMemoryStateStore {
    /// Creates an empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl RequestStateStore for InMemoryStateStore {
    fn get<'a>(&'a self, tag: &'a str) -> BoxFuture<'a, Option<Response>> {
        Box::pin(async move {
            let now = now_secs();
            // Read first; the `Ref` guard is dropped at the end of the `match`,
            // before any `remove`, so there is no self-deadlock on the shard.
            let hit = match self.entries.get(tag) {
                Some(entry) if entry.1 > now => Some(entry.0.clone()),
                Some(_) => None, // present but expired
                None => return None,
            };
            if hit.is_none() {
                self.entries.remove(tag);
            }
            hit
        })
    }

    fn put<'a>(&'a self, tag: &'a str, response: Response, exp: u64) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let now = now_secs();
            // Opportunistically drop expired entries so the map stays bounded.
            self.entries.retain(|_, (_, e)| *e > now);
            self.entries.insert(tag.to_owned(), (response, exp));
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RequestId;

    fn resp(id: i64) -> Response {
        Response::success(RequestId::Number(id), serde_json::json!({ "ok": true }))
    }

    #[tokio::test]
    async fn put_then_get_returns_the_cached_response() {
        let store = InMemoryStateStore::new();
        assert!(store.get("tag").await.is_none());

        store.put("tag", resp(1), now_secs() + 300).await;
        let got = store.get("tag").await.expect("cached response");
        assert_eq!(*got.id(), RequestId::Number(1));
    }

    #[tokio::test]
    async fn expired_entries_are_not_returned() {
        let store = InMemoryStateStore::new();
        // Already in the past.
        store
            .put("tag", resp(1), now_secs().saturating_sub(1))
            .await;
        assert!(store.get("tag").await.is_none());
    }

    #[tokio::test]
    async fn put_evicts_expired_entries() {
        let store = InMemoryStateStore::new();
        store
            .put("old", resp(1), now_secs().saturating_sub(1))
            .await;
        store.put("new", resp(2), now_secs() + 300).await;
        // The expired entry was swept by the second put.
        assert_eq!(store.entries.len(), 1);
        assert!(store.entries.contains_key("new"));
    }
}

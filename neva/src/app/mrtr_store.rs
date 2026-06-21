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
use std::sync::Arc;

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

    /// Atomically claims `tag` for an in-flight final round, returning a guard
    /// the caller holds across the [`get`](Self::get) check, handler execution
    /// and the committing [`put`](Self::put).
    ///
    /// This closes a race the cache alone cannot: two identical final-round
    /// retries for the same state (for example, an HTTP client that timed out
    /// and re-sent while the first round is still executing) can both miss
    /// [`get`](Self::get) before either reaches [`put`](Self::put), so both
    /// re-run the handler and re-drain its
    /// [`on_commit`](crate::Context::on_commit) effects — duplicating side
    /// effects such as charges. Holding a per-`tag` reservation across the whole
    /// section serialises them: the loser blocks until the winner has committed,
    /// then sees the cached response on its own [`get`](Self::get) and never
    /// re-executes.
    ///
    /// The returned guard releases the reservation on drop. The default
    /// implementation is a no-op (returns a guard that reserves nothing) so
    /// existing stores keep compiling; the default
    /// [`InMemoryStateStore`] overrides it with an in-process per-`tag` lock,
    /// sufficient for a single instance. **A shared store backing a
    /// multi-instance deployment must override this with a distributed lock**,
    /// for the same reason it must share the MRTR secret: a retry routed to a
    /// different instance must serialise against the round still committing
    /// elsewhere.
    fn reserve<'a>(&'a self, _tag: &'a str) -> BoxFuture<'a, Box<dyn Send>> {
        Box::pin(async { Box::new(()) as Box<dyn Send> })
    }
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
    /// Per-`tag` reservation locks (see [`RequestStateStore::reserve`]). Swept
    /// in [`put`](Self::put) once no task holds or awaits an entry, so the map
    /// tracks only in-flight final rounds.
    locks: dashmap::DashMap<String, Arc<tokio::sync::Mutex<()>>>,
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
            // Drop reservation locks no task holds or awaits — only the map's
            // own reference remains (`strong_count == 1`). The tag committing
            // here is still held by the live guard (`>= 2`), so it survives.
            self.locks.retain(|_, m| Arc::strong_count(m) > 1);
            self.entries.insert(tag.to_owned(), (response, exp));
        })
    }

    fn reserve<'a>(&'a self, tag: &'a str) -> BoxFuture<'a, Box<dyn Send>> {
        Box::pin(async move {
            // Get-or-create the per-tag mutex, then take it. `lock_owned`
            // consumes a clone of the `Arc` and the returned guard keeps it, so
            // the guard is `'static`. While any task holds or awaits the lock
            // the `Arc`'s strong count stays above 1, keeping it out of `put`'s
            // sweep until the last user is gone.
            let mutex = self
                .locks
                .entry(tag.to_owned())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone();
            let guard = mutex.lock_owned().await;
            Box::new(guard) as Box<dyn Send>
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

    #[tokio::test]
    async fn reserve_serialises_concurrent_final_rounds() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let store = Arc::new(InMemoryStateStore::new());
        let runs = Arc::new(AtomicUsize::new(0));

        // Mirrors the dispatch critical section: reserve, then cache-check, then
        // (on a miss) run the "handler" side effect and cache the result.
        async fn round(store: Arc<InMemoryStateStore>, runs: Arc<AtomicUsize>) {
            let _guard = store.reserve("tag").await;
            if store.get("tag").await.is_none() {
                runs.fetch_add(1, Ordering::SeqCst);
                // Force the holder across an await point so the other retry is
                // made to wait on the reservation rather than racing the cache.
                tokio::task::yield_now().await;
                store.put("tag", resp(1), now_secs() + 300).await;
            }
        }

        let a = tokio::spawn(round(store.clone(), runs.clone()));
        let b = tokio::spawn(round(store.clone(), runs.clone()));
        a.await.expect("task a");
        b.await.expect("task b");

        assert_eq!(
            runs.load(Ordering::SeqCst),
            1,
            "the final-round handler must run exactly once across identical retries"
        );
        assert!(store.get("tag").await.is_some());
    }

    #[tokio::test]
    async fn put_sweeps_released_reservation_locks() {
        let store = InMemoryStateStore::new();
        {
            let _guard = store.reserve("tag").await;
            assert_eq!(store.locks.len(), 1);
        }
        // The guard is dropped; the next put sweeps the now-idle lock entry.
        store.put("other", resp(1), now_secs() + 300).await;
        assert!(store.locks.is_empty());
    }
}

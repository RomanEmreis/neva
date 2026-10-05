//! Utilities for tracking requests

use crate::error::{Error, ErrorCode};
use crate::types::{RequestId, Response};
use dashmap::DashMap;
use std::{
    cmp::Ordering,
    collections::BinaryHeap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

const DEFAULT_REQUEST_TTL: Duration = Duration::from_secs(10);

/// Result sent through the internal pending-request channel.
///
/// This stays as an explicit enum instead of `Option<Response>` so the timeout
/// path is self-describing at the type level. The `Response` variant is large,
/// but this is the hot path and boxing would add a heap allocation to every
/// successful request; keep the full response inline and allow the size
/// difference intentionally.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub(crate) enum PendingResponse {
    /// A regular JSON-RPC response received from the peer.
    ///
    /// The inner `Response` is read by the client handler and by the server's
    /// `Context::send_request`; the latter is unused under the stateless 2026-07-28, so
    /// in a 2026-07-28 server build without the client this field is only written.
    #[cfg_attr(not(feature = "legacy-spec"), allow(dead_code))]
    Response(Response),
    /// A locally generated timeout for an expired pending request.
    Timeout,
}

impl PendingResponse {
    #[inline]
    #[cfg(test)]
    fn matches_timeout(&self) -> bool {
        matches!(self, Self::Timeout)
    }
}

/// Represents a request handle
pub(crate) struct RequestHandle {
    sender: oneshot::Sender<PendingResponse>,
    _cancellation_token: CancellationToken,
    expires_at: Option<Instant>,
    /// Which push made this entry, so a guard releases its own and never a
    /// later request that took the same id.
    slot: u64,
    /// Whether the request has gone out, so a response to it may still come.
    /// Set by [`RequestQueue::activate`].
    sent: bool,
}

/// Represents a request tracking "queue" that holds a hash map of [`oneshot::Sender`] for requests
/// that are awaiting responses.
#[derive(Clone)]
pub(crate) struct RequestQueue {
    pending: Arc<DashMap<RequestId, RequestHandle>>,
    expirations: Arc<Mutex<BinaryHeap<RequestExpiry>>>,
    // `next_expiry_seq` / `ttl` drive the TTL countdown started by `activate`,
    // which only runs for outbound requests (client, or legacy server callbacks).
    // A 2026-07-28 server build without the client never activates, leaving them unread.
    #[cfg_attr(not(feature = "legacy-spec"), allow(dead_code))]
    next_expiry_seq: Arc<AtomicU64>,
    /// Numbers each push; see [`RequestHandle::slot`].
    #[cfg_attr(not(feature = "legacy-spec"), allow(dead_code))]
    next_slot: Arc<AtomicU64>,
    #[cfg_attr(not(feature = "legacy-spec"), allow(dead_code))]
    ttl: Duration,
}

struct RequestExpiry {
    expires_at: Instant,
    sequence: u64,
    id: RequestId,
    /// The push this deadline belongs to, so the sweep never times out a
    /// later request that took the same id.
    slot: u64,
}

/// One push onto a [`RequestQueue`]: its id, and which push under that id.
///
/// An id alone does not name a slot for long. Once a response settles one, a
/// caller-chosen id can be taken again by another request, and anything still
/// holding only the id -- an activation running late, a subscription tearing
/// down -- would act on that newer request instead. Operations that act on a
/// request after its push take this, and touch the slot only if it is still
/// the one this push made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueuedRequest {
    id: RequestId,
    slot: u64,
}

impl PartialEq for RequestExpiry {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.expires_at == other.expires_at && self.sequence == other.sequence
    }
}

impl Eq for RequestExpiry {}

impl PartialOrd for RequestExpiry {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RequestExpiry {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .expires_at
            .cmp(&self.expires_at)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

impl RequestHandle {
    /// Creates a new [`RequestHandle`]
    #[cfg_attr(not(feature = "legacy-spec"), allow(dead_code))]
    pub(super) fn new(sender: oneshot::Sender<PendingResponse>, ttl: Duration) -> Self {
        Self {
            sender,
            _cancellation_token: CancellationToken::new(),
            expires_at: (!ttl.is_zero()).then_some(Instant::now() + ttl),
            slot: 0,
            sent: false,
        }
    }

    /// Sends a [`Response`] to MCP server
    pub(crate) fn send(self, resp: Response) {
        // Nobody waiting is an ordinary outcome: a request its caller gave up
        // on after it went out stays queued until its answer comes, and the
        // answer then has nowhere to go but here.
        if let Err(_resp) = self.sender.send(PendingResponse::Response(resp)) {
            #[cfg(feature = "tracing")]
            tracing::debug!(
                logger = "neva",
                "Discarding a response its caller gave up waiting for: {:?}",
                _resp
            );
        }
    }

    /// Whether this entry's TTL has run out.
    #[inline]
    fn is_expired(&self) -> bool {
        self.expires_at
            .is_some_and(|expires_at| expires_at <= Instant::now())
    }

    /// Completes the pending request with a timeout response.
    #[inline]
    pub(crate) fn send_timeout(self) {
        match self.sender.send(PendingResponse::Timeout) {
            Ok(_) => (),
            Err(_err) => {
                #[cfg(feature = "tracing")]
                tracing::error!(
                    logger = "neva",
                    "Request handler failed to send timeout response: {:?}",
                    _err
                );
            }
        };
    }
}

impl RequestQueue {
    /// Creates a new [`RequestQueue`] with the given entry TTL.
    #[inline]
    pub(crate) fn new(ttl: Duration) -> Self {
        Self {
            pending: Arc::new(DashMap::new()),
            expirations: Arc::new(Mutex::new(BinaryHeap::new())),
            next_expiry_seq: Arc::new(AtomicU64::new(0)),
            next_slot: Arc::new(AtomicU64::new(0)),
            ttl,
        }
    }

    /// Pushes a request with [`RequestId`] to the "queue"
    /// and returns a [`oneshot::Receiver`] for the response.
    ///
    /// Refuses an `id` a response may still arrive for: one that is waiting,
    /// and one its caller gave up on after it was sent. Two requests under one
    /// id would share one slot, and the first's response would be handed to
    /// the second caller. Ids this crate generates never repeat, but a caller's
    /// own -- a batch built by hand -- can repeat one, and with a client shared
    /// across tasks the two can be in flight together. An entry whose TTL has
    /// run out gives its id up: that is the point past which this client no
    /// longer expects an answer.
    ///
    /// Outbound-request path. Its callers are the client's `listen` (2026-07-28)
    /// and the legacy server's callbacks; a build with neither has none, and
    /// other client sends go through [`Self::push_guarded`].
    #[inline]
    #[cfg_attr(
        not(any(
            all(feature = "client", not(feature = "legacy-spec")),
            all(feature = "server", feature = "legacy-spec")
        )),
        allow(dead_code)
    )]
    pub(crate) fn push(
        &self,
        id: &RequestId,
    ) -> Result<(oneshot::Receiver<PendingResponse>, QueuedRequest), Error> {
        self.push_slot(id)
    }

    /// [`Self::push`], plus a guard that releases the slot when dropped.
    #[cfg(feature = "client")]
    #[inline]
    pub(crate) fn push_guarded(
        &self,
        id: &RequestId,
    ) -> Result<(oneshot::Receiver<PendingResponse>, QueuedRequestGuard<'_>), Error> {
        let (receiver, queued) = self.push_slot(id)?;
        Ok((
            receiver,
            QueuedRequestGuard {
                pending: &self.pending,
                queued,
            },
        ))
    }

    #[cfg_attr(not(feature = "legacy-spec"), allow(dead_code))]
    fn push_slot(
        &self,
        id: &RequestId,
    ) -> Result<(oneshot::Receiver<PendingResponse>, QueuedRequest), Error> {
        use dashmap::mapref::entry::Entry;

        let (sender, receiver) = oneshot::channel();
        let mut handle = RequestHandle::new(sender, self.ttl);
        handle.expires_at = None;
        let slot = self.next_slot.fetch_add(1, AtomicOrdering::Relaxed);
        handle.slot = slot;

        let replaced = match self.pending.entry(id.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(handle);
                None
            }
            Entry::Occupied(mut entry) if entry.get().is_expired() => Some(entry.insert(handle)),
            Entry::Occupied(entry) if entry.get().sender.is_closed() => {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    format!(
                        "Request id `{id}` was given up on after it was sent, and its response \
                         may still arrive; send the retry under a fresh id"
                    ),
                ));
            }
            Entry::Occupied(_) => {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    format!("Request id `{id}` is already waiting for a response"),
                ));
            }
        };
        // Outside the map's lock: whoever was waiting on the expired entry
        // learns it timed out.
        if let Some(expired) = replaced {
            expired.send_timeout();
        }

        Ok((
            receiver,
            QueuedRequest {
                id: id.clone(),
                slot,
            },
        ))
    }

    /// Starts the TTL countdown for a queued request after it has been sent,
    /// and records that it went out.
    ///
    /// Only the slot `queued` names: a response can settle it before the
    /// activation gets there, and a later request may have taken the id since.
    /// Marking that one sent would make its guard keep an id for an answer
    /// that is never coming.
    ///
    /// Companion to [`Self::push`]; see it for why this is unused by a 2026-07-28
    /// server build without the client.
    #[inline]
    #[cfg_attr(not(feature = "legacy-spec"), allow(dead_code))]
    pub(crate) fn activate(&self, queued: &QueuedRequest) {
        let id = &queued.id;
        if let Some(mut handle) = self.pending.get_mut(id)
            && handle.slot == queued.slot
        {
            handle.sent = true;
            // Always a deadline, a zero TTL included: a slot that went out is
            // kept for its answer (see `QueuedRequestGuard`), and unanswered,
            // nothing but the sweep would ever free it. A zero TTL makes the
            // deadline now, which is when the wait for the answer ends too.
            let expires_at = Instant::now() + self.ttl;
            handle.expires_at = Some(expires_at);
            drop(handle);

            let sequence = self.next_expiry_seq.fetch_add(1, AtomicOrdering::Relaxed);
            if let Ok(mut expirations) = self.expirations.lock() {
                expirations.push(RequestExpiry {
                    expires_at,
                    sequence,
                    id: id.clone(),
                    slot: queued.slot,
                });
            }
        }

        self.cleanup_expired();
    }

    /// Removes the slot `queued` names, if it is still that push's: the
    /// teardown of a request that will not be answered here, such as a
    /// subscription that ended or a wait that timed out.
    ///
    /// Callers are the same as [`Self::push`]'s.
    #[inline]
    #[cfg_attr(
        not(any(
            all(feature = "client", not(feature = "legacy-spec")),
            all(feature = "server", feature = "legacy-spec")
        )),
        allow(dead_code)
    )]
    pub(crate) fn release(&self, queued: &QueuedRequest) {
        self.pending
            .remove_if(&queued.id, |_, handle| handle.slot == queued.slot);
    }

    /// Pops the [`RequestHandle`] by [`RequestId`] and removes it from the queue
    #[inline]
    pub(crate) fn pop(&self, id: &RequestId) -> Option<RequestHandle> {
        if self.is_expired(id) {
            if let Some((_, handle)) = self.pending.remove(id) {
                handle.send_timeout();
            }
            return None;
        }

        self.pending.remove(id).map(|(_, handle)| handle)
    }

    /// Returns how many requests are currently queued.
    ///
    /// Slots for long-lived requests (`subscriptions/listen`) carry no TTL, so
    /// nothing sweeps them: whoever gives up on one has to release it. This is
    /// how the tests hold that contract.
    #[inline]
    // Its callers are the subscription tests, which the legacy profile compiles
    // out; the profile that keeps them is the one that needs this.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.pending.len()
    }

    /// Drops every queued request, closing the receiver each one is awaited on.
    ///
    /// For when no response can arrive any more -- the receive loop has stopped
    /// because the transport is gone. Waiting out the TTL would be pointless
    /// for ordinary requests and endless for a `subscriptions/listen` slot,
    /// which has no TTL at all: its holder would await a stream end that is
    /// never coming.
    #[inline]
    // Its only caller is the client's receive loop; a server-only build has no
    // receive loop to stop.
    #[cfg_attr(not(feature = "client"), allow(dead_code))]
    pub(crate) fn abandon_all(&self) {
        self.pending.clear();
        if let Ok(mut expirations) = self.expirations.lock() {
            expirations.clear();
        }
    }

    /// Takes a [`Response`] and completes the request if it's still pending
    #[inline]
    pub(crate) fn complete(&self, resp: Response) {
        self.cleanup_expired();

        if let Some(sender) = self.pop(&resp.full_id()) {
            sender.send(resp)
        }
    }

    #[inline]
    fn cleanup_expired(&self) {
        let now = Instant::now();
        let mut expired = Vec::new();

        if let Ok(mut expirations) = self.expirations.lock() {
            while expirations
                .peek()
                .is_some_and(|entry| entry.expires_at <= now)
            {
                let entry = expirations.pop().expect("peeked entry must exist");
                expired.push((entry.id, entry.slot));
            }
        }

        for (id, slot) in expired {
            if let Some((_, handle)) = self.pending.remove_if(&id, |_, handle| handle.slot == slot)
            {
                handle.send_timeout();
            }
        }
    }

    #[inline]
    fn is_expired(&self, id: &RequestId) -> bool {
        self.pending
            .get(id)
            .is_some_and(|handle| handle.is_expired())
    }
}

impl Default for RequestQueue {
    #[inline]
    fn default() -> Self {
        Self::new(DEFAULT_REQUEST_TTL)
    }
}

/// Settles a request's slot when the wait for its response ends, however it
/// ends.
///
/// The wait can end without any of its own code running: a caller that drops
/// the future (an outer `timeout`, a lost `select!` branch) stops it at its
/// last suspension point. What the slot needs then depends on whether the
/// request went out:
///
/// - **Never sent** -- a refused id, a batch that could not be built, a failed
///   write. No answer can come, so the slot is released at once.
/// - **Sent.** The server may still answer, and an answer matched by id alone
///   would be handed to whichever request holds that id next. So the entry
///   stays, its waiter gone, until the answer arrives and is discarded or the
///   TTL sweeps it; until then [`RequestQueue::push`] refuses the id. Freeing
///   it sooner would trade a refused retry for a misrouted response.
///
/// Releasing a slot the response already completed is a no-op, and so is
/// releasing one a later request has since taken under the same id: the guard
/// touches the entry its own push made and nothing else.
///
/// It borrows the queue for as long as the wait lasts and owns its id, so it
/// can travel with whatever awaits the response: a batch hands each request's
/// guard to the future collecting that request's reply. Its envelopes have
/// gone into the batch by then, so there is no id left to borrow.
///
/// The HTTP server's `PendingSlot` guards a different map: the transport's
/// POST-to-reply routes, which it fills itself and can hand over to a streamed
/// body. This one guards an entry [`RequestQueue::push_guarded`] made and
/// never transfers.
#[cfg(feature = "client")]
pub(crate) struct QueuedRequestGuard<'a> {
    pending: &'a DashMap<RequestId, RequestHandle>,
    queued: QueuedRequest,
}

#[cfg(feature = "client")]
impl QueuedRequestGuard<'_> {
    /// The push whose slot this guards.
    #[inline]
    pub(crate) fn queued(&self) -> &QueuedRequest {
        &self.queued
    }
}

#[cfg(feature = "client")]
impl Drop for QueuedRequestGuard<'_> {
    #[inline]
    fn drop(&mut self) {
        self.pending.remove_if(&self.queued.id, |_, handle| {
            handle.slot == self.queued.slot && !handle.sent
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::time::{Duration, timeout};

    /// Two requests waiting under one id would share one slot: the second would
    /// replace the first's waiter and be handed the first's response.
    #[test]
    fn an_id_already_waiting_is_refused() {
        let queue = RequestQueue::default();
        let id = RequestId::Number(7);

        let (_first, _) = queue.push(&id).expect("a fresh id");
        let err = queue.push(&id).expect_err("the id is in flight");
        assert_eq!(err.code, ErrorCode::InvalidRequest);

        assert!(queue.pop(&id).is_some());
        assert!(
            queue.push(&id).is_ok(),
            "once answered, the id is free again"
        );
    }

    /// An entry whose TTL ran out is no longer waited on: its id can be taken,
    /// and its waiter hears that it timed out.
    #[tokio::test]
    async fn an_expired_id_is_given_up() {
        let queue = RequestQueue::new(Duration::from_millis(1));
        let id = RequestId::Number(7);

        let (stale, id_slot) = queue.push(&id).expect("a fresh id");
        queue.activate(&id_slot);
        tokio::time::sleep(Duration::from_millis(5)).await;

        let (_fresh, _) = queue.push(&id).expect("an expired entry gives its id up");
        assert!(matches!(stale.await, Ok(PendingResponse::Timeout)));
    }

    /// A request given up on after it went out may still be answered, so its
    /// id stays taken until the answer comes -- and the answer is discarded,
    /// not handed to whoever asks under that id next.
    #[cfg(feature = "client")]
    #[test]
    fn a_request_given_up_after_it_went_out_keeps_its_id() {
        let queue = RequestQueue::default();
        let id = RequestId::Number(7);

        let (rx, guard) = queue.push_guarded(&id).expect("a fresh id");
        queue.activate(guard.queued());
        drop(rx);
        drop(guard);

        let err = queue.push(&id).expect_err("its answer may still arrive");
        assert!(err.to_string().contains("fresh id"), "{err}");

        queue.complete(Response::success(id.clone(), json!({ "late": true })));
        assert_eq!(queue.len(), 0, "the late answer settles the slot");
        assert!(queue.push(&id).is_ok(), "and frees the id");
    }

    /// A slot that went out always gets a deadline: unanswered, nothing else
    /// would ever free it. With a zero TTL the deadline is now -- the wait for
    /// the answer is already over -- so the id comes back, and the slots of
    /// abandoned requests do not pile up.
    #[cfg(feature = "client")]
    #[test]
    fn a_sent_slot_expires_even_with_a_zero_ttl() {
        let queue = RequestQueue::new(Duration::ZERO);
        let id = RequestId::Number(7);

        let (rx, guard) = queue.push_guarded(&id).expect("a fresh id");
        queue.activate(guard.queued());
        drop(rx);
        drop(guard);

        let (_rx, next) = queue.push(&id).expect("an expired slot gives its id up");
        queue.activate(&next);
        assert_eq!(queue.len(), 0, "the sweep frees what has expired");
    }

    /// An activation can run late: a fast answer settles its slot first, and a
    /// later request takes the id before the activation gets there. Marking
    /// that one sent would make its guard keep the id for an answer that is
    /// never coming, should it then fail to go out.
    #[cfg(feature = "client")]
    #[test]
    fn a_late_activation_leaves_a_newer_slot_alone() {
        let queue = RequestQueue::default();
        let id = RequestId::Number(7);

        let (_rx, first) = queue.push(&id).expect("a fresh id");
        queue.complete(Response::success(id.clone(), json!({})));
        let (_rx, second) = queue.push_guarded(&id).expect("the id is free again");

        queue.activate(&first);
        drop(second);

        assert_eq!(
            queue.len(),
            0,
            "a request that never went out leaves nothing"
        );
        assert!(queue.push(&id).is_ok());
    }

    /// A guard releases the entry its own push made. By the time it drops, its
    /// response may have arrived and a later request may have taken the id;
    /// that request's slot is not the guard's to release.
    #[cfg(feature = "client")]
    #[test]
    fn a_guard_releases_only_its_own_slot() {
        let queue = RequestQueue::default();
        let id = RequestId::Number(7);

        let (_rx, guard) = queue.push_guarded(&id).expect("a fresh id");
        assert!(queue.pop(&id).is_some(), "the response arrives");
        let (_next, _) = queue.push(&id).expect("the id is free again");

        drop(guard);
        assert_eq!(queue.len(), 1, "the later request keeps its slot");

        let (_rx, guard) = queue
            .push_guarded(&RequestId::Number(8))
            .expect("a fresh id");
        drop(guard);
        assert_eq!(
            queue.len(),
            1,
            "an abandoned request gives its own slot back"
        );
    }

    #[test]
    fn it_pushes_and_pops_request() {
        let queue = RequestQueue::default();
        let id = RequestId::Number(1);

        let (receiver, _) = queue.push(&id).expect("a fresh id");
        let handle = queue.pop(&id);

        assert!(handle.is_some(), "Expected handle to exist");
        assert!(
            queue.pop(&id).is_none(),
            "Handle should be removed after pop"
        );

        drop(receiver); // Avoid warning for unused receiver
    }

    #[tokio::test]
    async fn it_sends_and_receives() {
        let queue = RequestQueue::default();
        let id = RequestId::Number(1);

        let (receiver, _) = queue.push(&id).expect("a fresh id");
        let handle = queue.pop(&id).expect("Should have handle");

        let expected = Response::success(id, json!({ "content": "done" }));
        handle.send(expected.clone());
        let Response::Ok(expected) = expected else {
            unreachable!()
        };

        let PendingResponse::Response(Response::Ok(actual)) =
            timeout(Duration::from_secs(1), receiver)
                .await
                .expect("Receiver should complete")
                .expect("Sender should send")
        else {
            unreachable!()
        };

        assert_eq!(actual.result, expected.result);
        assert_eq!(actual.id, expected.id);
    }

    #[tokio::test]
    async fn it_sends_response_if_pending() {
        let queue = RequestQueue::default();
        let id = RequestId::Number(1);

        let (receiver, _) = queue.push(&id).expect("a fresh id");

        let response = Response::success(id, json!({ "content": "done" }));
        queue.complete(response.clone());

        let Response::Ok(response) = response else {
            unreachable!()
        };

        let PendingResponse::Response(Response::Ok(actual)) =
            timeout(Duration::from_secs(1), receiver)
                .await
                .expect("Should receive within timeout")
                .expect("Should receive response")
        else {
            unreachable!()
        };

        assert_eq!(actual.result, response.result);
    }

    #[test]
    fn it_does_nothing_if_not_pending() {
        let queue = RequestQueue::default();
        let id = RequestId::Number(1);

        let response = Response::success(id, json!({ "content": "done" }));

        // No push before complete
        queue.complete(response);

        // Nothing to assert really, just verifying it doesn't panic or error
    }

    #[test]
    fn it_does_remove_expired_pending_requests() {
        let queue = RequestQueue::new(Duration::from_millis(1));
        let id = RequestId::Number(1);

        let (_receiver, id_slot) = queue.push(&id).expect("a fresh id");
        queue.activate(&id_slot);
        std::thread::sleep(Duration::from_millis(10));

        assert!(queue.pop(&id).is_none());
    }

    #[tokio::test]
    async fn pop_does_not_close_non_target_receivers() {
        let queue = RequestQueue::new(Duration::from_millis(5));
        let expired_id = RequestId::Number(1);
        let live_id = RequestId::Number(2);

        let (_expired, expired_id_slot) = queue.push(&expired_id).expect("a fresh id");
        let (live, _) = queue.push(&live_id).expect("a fresh id");
        queue.activate(&expired_id_slot);

        std::thread::sleep(Duration::from_millis(10));

        assert!(queue.pop(&expired_id).is_none());

        let response = Response::success(live_id, json!({ "content": "done" }));
        queue.complete(response);

        assert!(
            timeout(Duration::from_secs(1), live).await.is_ok(),
            "non-target receiver should remain open"
        );
    }

    #[tokio::test]
    async fn pop_sends_timeout_response_for_expired_request() {
        let queue = RequestQueue::new(Duration::from_millis(5));
        let id = RequestId::Number(1);

        let (receiver, id_slot) = queue.push(&id).expect("a fresh id");
        queue.activate(&id_slot);

        std::thread::sleep(Duration::from_millis(10));

        assert!(queue.pop(&id).is_none());

        assert!(
            receiver
                .await
                .expect("expired request should resolve")
                .matches_timeout(),
            "expired request should resolve as timeout"
        );
    }

    #[tokio::test]
    async fn cleanup_sends_timeout_response_for_expired_requests() {
        let queue = RequestQueue::new(Duration::from_millis(5));
        let expired_id = RequestId::Number(1);
        let live_id = RequestId::Number(2);

        let (expired, expired_id_slot) = queue.push(&expired_id).expect("a fresh id");
        let (_live, _) = queue.push(&live_id).expect("a fresh id");
        queue.activate(&expired_id_slot);

        std::thread::sleep(Duration::from_millis(10));

        let response = Response::success(live_id, json!({ "content": "done" }));
        queue.complete(response);

        assert!(
            expired
                .await
                .expect("expired request should resolve")
                .matches_timeout(),
            "expired request should resolve as timeout"
        );
    }

    #[test]
    fn push_does_not_start_ttl_until_activated() {
        let queue = RequestQueue::new(Duration::from_millis(1));
        let id = RequestId::Number(1);

        let (_receiver, _) = queue.push(&id).expect("a fresh id");
        std::thread::sleep(Duration::from_millis(10));

        assert!(queue.pop(&id).is_some());
    }
}

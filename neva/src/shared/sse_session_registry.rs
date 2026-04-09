//! SSE session registry for server-side Last-Event-ID replay

use crate::{
    error::{Error, ErrorCode},
    types::Message,
};
use dashmap::DashMap;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc::{Sender, error::TrySendError};
use uuid::Uuid;

/// Bounded in-memory registry providing SSE event buffering and replay for
/// server-side Last-Event-ID support.
pub(crate) struct SseSessionRegistry {
    capacity: usize,
    sessions: DashMap<Uuid, SseSession>,
    next_gen: AtomicU64,
}

struct SseSession {
    sender: Sender<(u64, Arc<Message>)>,
    buffer: Mutex<VecDeque<(u64, Arc<Message>)>>,
    last_activity: Mutex<Instant>,
    /// Relaxed ordering: single-writer (dispatch loop); Mutex on buffer provides
    /// happens-before for all readers.
    next_seq: AtomicU64,
    capacity: usize,
    /// Updated on each reconnect via register(). Plain u64 — mutated only through
    /// a DashMap write-shard lock.
    generation: u64,
}

impl SseSessionRegistry {
    fn disconnected_sender() -> Sender<(u64, Arc<Message>)> {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(rx);
        tx
    }

    /// Creates a new [`SseSessionRegistry`].
    ///
    /// `capacity` is the maximum number of events buffered per session.
    /// `0` disables buffering (events still flow live; `replay_since` always returns empty).
    ///
    /// # Example
    /// ```rust,ignore
    /// let registry = SseSessionRegistry::new(64);
    /// ```
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            sessions: DashMap::new(),
            next_gen: AtomicU64::new(0),
        }
    }

    /// Registers or updates the live sender for a session.
    ///
    /// For an **existing** session ID: updates `sender` and `generation` in place,
    /// preserving `buffer` and `next_seq` (replay continuity across reconnects).
    /// For a **new** session ID: creates a fresh `SseSession`.
    ///
    /// Returns the new generation number. The caller must pass this value to
    /// [`unregister`] when the connection ends.
    pub(crate) fn register(&self, id: Uuid, sender: Sender<(u64, Arc<Message>)>) -> u64 {
        let generation = self.next_gen.fetch_add(1, Ordering::Relaxed) + 1;
        let now = Instant::now();

        self.sessions
            .entry(id)
            .and_modify(|s| {
                // `sender.clone()` is forced by the DashMap entry API: `or_insert_with`
                // consumes `sender` by move, so `and_modify` must clone.
                s.sender = sender.clone();
                s.generation = generation;
                *s.last_activity.lock().unwrap_or_else(|e| e.into_inner()) = now;
            })
            .or_insert_with(|| SseSession {
                sender,
                buffer: Mutex::new(VecDeque::new()),
                last_activity: Mutex::new(now),
                next_seq: AtomicU64::new(0),
                capacity: self.capacity,
                generation,
            });

        generation
    }

    /// Disconnects the live sender only if the stored generation matches `generation`.
    ///
    /// No-op when the session has been re-registered with a newer generation, preventing
    /// stale cleanup from disconnecting a live reconnected session. Buffered replay state is
    /// preserved so clients can resume after a transient GET drop.
    pub(crate) fn unregister(&self, id: &Uuid, generation: u64) {
        if let Some(mut session) = self.sessions.get_mut(id)
            && session.generation == generation
        {
            session.sender = Self::disconnected_sender();
            *session
                .last_activity
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Instant::now();
        }
    }

    /// Unconditionally removes a session.
    ///
    /// Use for explicit session termination (e.g. DELETE /mcp). Unlike [`unregister`],
    /// this does not check the generation — the session is always removed.
    pub(crate) fn terminate(&self, id: &Uuid) {
        self.sessions.remove(id);
    }

    /// Buffers `message` and sends it to the session's live channel.
    ///
    /// Buffer-first: the message is stored before the channel send, so a dead
    /// channel does not lose the event — it remains available for the next reconnect.
    /// If the session is not found, the event is dropped (no buffer to write to).
    pub(crate) fn send(&self, message: Message) -> Result<(), Error> {
        let Some(&session_id) = message.session_id() else {
            return Err(Error::new(ErrorCode::InvalidParams, "missing session id"));
        };

        let Some(mut session) = self.sessions.get_mut(&session_id) else {
            #[cfg(feature = "tracing")]
            tracing::warn!(
                logger = "neva",
                "Session {} not found for SSE send — event dropped",
                session_id
            );
            return Ok(());
        };

        let arc = Arc::new(message);
        let seq = session.next_seq.fetch_add(1, Ordering::Relaxed);
        *session
            .last_activity
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Instant::now();

        if session.capacity > 0 {
            let mut buf = session.buffer.lock().unwrap_or_else(|e| e.into_inner());
            buf.push_back((seq, arc.clone()));
            while buf.len() > session.capacity {
                buf.pop_front();
            }
        }

        match session.sender.try_send((seq, arc)) {
            Ok(()) => {}
            Err(TrySendError::Full((_seq, _arc))) => {
                session.sender = Self::disconnected_sender();
                #[cfg(feature = "tracing")]
                {
                    crate::types::notification::fmt::LOG_REGISTRY.unregister(&session_id);
                    tracing::warn!(
                        logger = "neva",
                        "Lagging SSE client for session {}: disconnecting SSE stream at seq={}",
                        session_id,
                        seq
                    );
                }
            }
            Err(TrySendError::Closed((_seq, _arc))) => {
                session.sender = Self::disconnected_sender();
                #[cfg(feature = "tracing")]
                {
                    crate::types::notification::fmt::LOG_REGISTRY.unregister(&session_id);
                    tracing::warn!(
                        logger = "neva",
                        "Dead channel for session {}: seq={} is in buffer for next reconnect",
                        session_id,
                        seq
                    );
                }
            }
        }

        Ok(())
    }

    /// Returns buffered events with `seq > last_seq` for replay after reconnect.
    ///
    /// If `last_seq` was evicted (oldest buffered seq > `last_seq`), the full buffer
    /// is returned (best-effort replay). Returns empty if the session is unknown,
    /// the buffer is empty, or `last_seq` equals the newest buffered seq.
    pub(crate) fn replay_since(&self, id: &Uuid, last_seq: u64) -> Vec<(u64, Arc<Message>)> {
        let Some(session) = self.sessions.get(id) else {
            return Vec::new();
        };

        let buf = session.buffer.lock().unwrap_or_else(|e| e.into_inner());
        if buf.is_empty() {
            return Vec::new();
        }

        // Eviction path: oldest seq was evicted → best-effort, return full buffer
        if buf.front().is_some_and(|(s, _)| *s > last_seq) {
            return buf.iter().cloned().collect();
        }

        buf.iter().filter(|(s, _)| *s > last_seq).cloned().collect()
    }

    /// Returns all buffered events in sequence order.
    ///
    /// Used on an initial SSE connection (no `Last-Event-ID`) to recover any events
    /// buffered during the POST → GET handshake window. Returns empty if the session
    /// is unknown or the buffer is empty.
    pub(crate) fn replay_all(&self, id: &Uuid) -> Vec<(u64, Arc<Message>)> {
        let Some(session) = self.sessions.get(id) else {
            return Vec::new();
        };
        let buf = session.buffer.lock().unwrap_or_else(|e| e.into_inner());
        buf.iter().cloned().collect()
    }

    /// Creates a buffer-only session entry if one does not already exist.
    ///
    /// Call when a session ID is first minted (on POST /mcp) so that any server-initiated
    /// events emitted before the client's SSE GET arrive are buffered and available for
    /// replay. If an entry already exists (live connection or prior pre-registration),
    /// this is a no-op — the existing buffer and sequence counter are preserved.
    ///
    /// Has no effect when `capacity == 0` (buffering disabled).
    pub(crate) fn pre_register(&self, id: Uuid) {
        if self.capacity == 0 {
            return;
        }
        self.sessions.entry(id).or_insert_with(|| SseSession {
            sender: Self::disconnected_sender(),
            buffer: Mutex::new(VecDeque::new()),
            last_activity: Mutex::new(Instant::now()),
            next_seq: AtomicU64::new(0),
            capacity: self.capacity,
            generation: 0,
        });
    }

    /// Removes disconnected sessions whose last activity is older than `ttl`.
    pub(crate) fn evict_stale(&self, ttl: Duration) {
        let now = Instant::now();
        let stale_ids: Vec<Uuid> = self
            .sessions
            .iter()
            .filter_map(|entry| {
                let last_activity = *entry
                    .last_activity
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                (entry.sender.is_closed() && now.duration_since(last_activity) >= ttl)
                    .then_some(*entry.key())
            })
            .collect();

        for id in stale_ids {
            self.sessions.remove(&id);
            #[cfg(feature = "tracing")]
            crate::types::notification::fmt::LOG_REGISTRY.unregister(&id);
        }
    }
}

impl std::fmt::Debug for SseSessionRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SseSessionRegistry")
            .field("capacity", &self.capacity)
            .field("sessions", &self.sessions.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::notification::Notification;
    use tokio::sync::mpsc;

    fn make_msg(session_id: Uuid) -> Message {
        Message::Notification(Notification::new("test", None)).set_session_id(session_id)
    }

    #[test]
    fn it_returns_generation_1_for_first_registration() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (tx, _rx) = mpsc::channel(8);
        let generation = registry.register(id, tx);
        assert_eq!(generation, 1);
    }

    #[test]
    fn it_returns_higher_generation_on_re_register() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (tx1, _rx1) = mpsc::channel(8);
        let (tx2, _rx2) = mpsc::channel(8);
        let gen1 = registry.register(id, tx1);
        let gen2 = registry.register(id, tx2);
        assert!(
            gen2 > gen1,
            "second registration must have higher generation"
        );
    }

    #[test]
    fn it_preserves_buffer_and_next_seq_on_re_register() {
        let registry = SseSessionRegistry::new(16);
        let id = Uuid::new_v4();

        // First connection: send 3 events → seqs 0, 1, 2
        let (tx1, _rx1) = mpsc::channel(16);
        registry.register(id, tx1);
        for _ in 0..3 {
            registry.send(make_msg(id)).unwrap();
        }

        // Reconnect: register new sender. Must NOT reset next_seq.
        let (tx2, mut rx2) = mpsc::channel(16);
        registry.register(id, tx2);

        // Post-reconnect events must continue from seq=3, not reset to 0.
        registry.send(make_msg(id)).unwrap();
        registry.send(make_msg(id)).unwrap();

        let post_seqs: Vec<u64> = std::iter::from_fn(|| rx2.try_recv().ok())
            .map(|(s, _)| s)
            .collect();
        assert_eq!(
            post_seqs,
            vec![3, 4],
            "seq must continue after reconnect, not reset to 0"
        );

        // Buffer still contains the post-reconnect events for replay
        let replayed = registry.replay_since(&id, 2);
        assert_eq!(replayed.len(), 2);
        assert_eq!(replayed[0].0, 3);
        assert_eq!(replayed[1].0, 4);
    }

    #[test]
    fn it_delivers_live_events_to_latest_sender_after_reregister() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();

        let (tx1, mut rx1) = mpsc::channel(8);
        registry.register(id, tx1);

        let (tx2, mut rx2) = mpsc::channel(8);
        registry.register(id, tx2);

        registry.send(make_msg(id)).unwrap();

        // New sender (rx2) gets it; old sender (rx1) does not
        assert!(rx2.try_recv().is_ok(), "new sender must receive event");
        assert!(rx1.try_recv().is_err(), "old sender must not receive event");
    }

    #[test]
    fn it_disconnects_session_when_generation_matches() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (tx, mut rx) = mpsc::channel(8);
        let generation = registry.register(id, tx);
        registry.unregister(&id, generation);

        registry.send(make_msg(id)).unwrap();
        assert!(rx.try_recv().is_err(), "live sender must be disconnected");
        assert_eq!(
            registry.replay_all(&id).len(),
            1,
            "buffer must be preserved"
        );
    }

    #[test]
    fn it_does_not_disconnect_session_when_generation_is_stale() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();

        let (tx1, _rx1) = mpsc::channel(8);
        let gen1 = registry.register(id, tx1);

        // Reconnect
        let (tx2, mut rx2) = mpsc::channel(8);
        registry.register(id, tx2);

        // Old generation unregister must be a no-op
        registry.unregister(&id, gen1);

        // New sender still receives events
        registry.send(make_msg(id)).unwrap();
        assert!(rx2.try_recv().is_ok(), "registration must be preserved");
    }

    #[test]
    fn it_terminates_session_unconditionally() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (tx, _rx) = mpsc::channel(8);
        registry.register(id, tx);
        registry.terminate(&id);
        assert!(registry.replay_since(&id, 0).is_empty());
    }

    #[test]
    fn it_buffers_event_and_delivers_to_channel() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (tx, mut rx) = mpsc::channel(8);
        registry.register(id, tx);

        // Send first event: seq=0
        registry.send(make_msg(id)).unwrap();
        let (seq, _) = rx.try_recv().expect("event must be delivered live");
        assert_eq!(seq, 0);

        // Send second event: seq=1. replay_since(&id, 0) returns seq > 0 → [seq=1]
        registry.send(make_msg(id)).unwrap();
        let replayed = registry.replay_since(&id, 0);
        assert_eq!(replayed.len(), 1);
        assert_eq!(replayed[0].0, 1);
    }

    #[test]
    fn it_shares_arc_allocation_between_buffer_and_channel() {
        let registry2 = SseSessionRegistry::new(1);
        let id2 = Uuid::new_v4();
        let (tx2, mut rx2) = mpsc::channel(1);
        registry2.register(id2, tx2);
        registry2.send(make_msg(id2)).unwrap(); // seq=0, capacity=1 so buf=[seq=0]

        let (_, arc_live) = rx2.try_recv().unwrap();
        // Arc strong_count = 2 means both buffer and channel hold a ref to the same allocation.
        assert_eq!(
            Arc::strong_count(&arc_live),
            2, // 1 from channel (arc_live) + 1 still in buffer
            "buffer and channel must share one Arc allocation"
        );
    }

    #[test]
    fn it_evicts_oldest_event_when_buffer_is_at_capacity() {
        let registry = SseSessionRegistry::new(3);
        let id = Uuid::new_v4();
        let (tx, _rx) = mpsc::channel(3);
        registry.register(id, tx);

        // Send 4 events into a capacity-3 buffer
        for _ in 0..4 {
            registry.send(make_msg(id)).unwrap();
        }

        // Buffer should hold seqs 1, 2, 3 (seq 0 evicted)
        // Eviction path: oldest_seq(1) > last_seq(0) → return full buffer
        let replayed = registry.replay_since(&id, 0);
        assert_eq!(replayed.len(), 3);
        assert_eq!(replayed[0].0, 1);
        assert_eq!(replayed[2].0, 3);
    }

    #[test]
    fn it_returns_empty_replay_when_capacity_is_zero() {
        let registry = SseSessionRegistry::new(0);
        let id = Uuid::new_v4();
        let (tx, mut rx) = mpsc::channel(1);
        registry.register(id, tx);

        registry.send(make_msg(id)).unwrap();

        // Event still delivered live
        assert!(rx.try_recv().is_ok());
        // But buffer is always empty
        assert!(registry.replay_since(&id, 0).is_empty());
    }

    #[test]
    fn it_returns_events_strictly_after_last_seq() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (tx, _rx) = mpsc::channel(8);
        registry.register(id, tx);

        for _ in 0..5 {
            registry.send(make_msg(id)).unwrap();
        }
        // seqs 0..=4 in buffer
        let replayed = registry.replay_since(&id, 2);
        assert_eq!(replayed.len(), 2); // seqs 3, 4
        assert_eq!(replayed[0].0, 3);
        assert_eq!(replayed[1].0, 4);
    }

    #[test]
    fn it_returns_full_buffer_when_last_seq_is_evicted() {
        let registry = SseSessionRegistry::new(3);
        let id = Uuid::new_v4();
        let (tx, _rx) = mpsc::channel(3);
        registry.register(id, tx);

        for _ in 0..5 {
            registry.send(make_msg(id)).unwrap();
        }
        // Buffer holds seqs 2, 3, 4 (seqs 0 and 1 were evicted).
        // Client sends last_seq=0 (evicted) → oldest(2) > 0 → full buffer returned.
        let replayed = registry.replay_since(&id, 0);
        assert_eq!(replayed.len(), 3);
        assert_eq!(replayed[0].0, 2);
    }

    #[test]
    fn it_returns_empty_when_last_seq_equals_newest() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (tx, _rx) = mpsc::channel(8);
        registry.register(id, tx);

        for _ in 0..3 {
            registry.send(make_msg(id)).unwrap();
        }
        // Newest seq = 2; replay_since(2) → strictly > 2 → empty
        let replayed = registry.replay_since(&id, 2);
        assert!(replayed.is_empty());
    }

    #[test]
    fn it_returns_empty_for_unknown_session() {
        let registry = SseSessionRegistry::new(8);
        assert!(registry.replay_since(&Uuid::new_v4(), 0).is_empty());
    }

    #[test]
    fn it_still_buffers_when_channel_is_dead() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (tx, rx) = mpsc::channel(8);
        registry.register(id, tx);
        drop(rx); // kill the channel

        // send() must not return an error — event is buffered for next reconnect
        registry.send(make_msg(id)).unwrap();

        // Buffer holds the event
        registry.send(make_msg(id)).unwrap(); // seq=1
        let replayed = registry.replay_since(&id, 0); // seq > 0
        assert_eq!(replayed.len(), 1);
        assert_eq!(replayed[0].0, 1);
    }

    #[test]
    fn it_produces_contiguous_seq_across_reconnects() {
        let registry = SseSessionRegistry::new(16);
        let id = Uuid::new_v4();

        let (tx1, _rx1) = mpsc::channel(16);
        registry.register(id, tx1);
        for _ in 0..5 {
            registry.send(make_msg(id)).unwrap();
        }
        // seqs 0..=4

        let (tx2, mut rx2) = mpsc::channel(16);
        registry.register(id, tx2);
        for _ in 0..5 {
            registry.send(make_msg(id)).unwrap();
        }
        // seqs 5..=9

        // Drain rx2 and confirm seqs are 5..=9
        let mut seqs: Vec<u64> = Vec::new();
        while let Ok((seq, _)) = rx2.try_recv() {
            seqs.push(seq);
        }
        assert_eq!(seqs, vec![5, 6, 7, 8, 9]);
    }

    #[test]
    fn it_buffers_events_during_pre_registration_window() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();

        // Simulate POST /initialize: session minted, SSE GET not yet arrived
        registry.pre_register(id);

        // Events sent before the SSE GET are buffered (dead channel, not an error)
        registry.send(make_msg(id)).unwrap(); // seq=0
        registry.send(make_msg(id)).unwrap(); // seq=1

        // Simulate GET /mcp: register live channel — in-place, preserving buffer
        let (tx, mut rx) = mpsc::channel(8);
        registry.register(id, tx);

        // All buffered events are available for replay
        let all = registry.replay_all(&id);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].0, 0);
        assert_eq!(all[1].0, 1);

        // Post-handshake events continue from seq=2 and are delivered live
        registry.send(make_msg(id)).unwrap(); // seq=2
        let (seq, _) = rx.try_recv().expect("seq=2 must be delivered live");
        assert_eq!(seq, 2);
    }

    #[test]
    fn it_pre_register_is_noop_when_session_already_registered() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();

        let (tx, mut rx) = mpsc::channel(8);
        registry.register(id, tx);
        registry.send(make_msg(id)).unwrap(); // seq=0

        // pre_register must not disturb the existing live session
        registry.pre_register(id);

        registry.send(make_msg(id)).unwrap(); // seq=1
        let seqs: Vec<u64> = std::iter::from_fn(|| rx.try_recv().ok())
            .map(|(s, _)| s)
            .collect();
        assert_eq!(seqs, vec![0, 1]);
    }

    #[test]
    fn it_pre_register_is_noop_when_capacity_is_zero() {
        let registry = SseSessionRegistry::new(0);
        let id = Uuid::new_v4();
        registry.pre_register(id); // must not create an entry
        assert!(registry.replay_all(&id).is_empty());
    }

    #[test]
    fn it_returns_all_buffered_events() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (tx, _rx) = mpsc::channel(8);
        registry.register(id, tx);

        for _ in 0..3 {
            registry.send(make_msg(id)).unwrap();
        }

        let all = registry.replay_all(&id);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].0, 0);
        assert_eq!(all[2].0, 2);
    }

    #[test]
    fn it_returns_empty_replay_all_for_unknown_session() {
        let registry = SseSessionRegistry::new(8);
        assert!(registry.replay_all(&Uuid::new_v4()).is_empty());
    }

    #[test]
    fn it_disconnects_live_stream_when_queue_fills() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (tx, mut rx) = mpsc::channel(1);
        registry.register(id, tx);

        registry.send(make_msg(id)).unwrap(); // fills live queue with seq=0
        registry.send(make_msg(id)).unwrap(); // seq=1 disconnects the live queue

        let (seq, _) = rx.try_recv().expect("first event must remain queued");
        assert_eq!(seq, 0);
        assert!(
            rx.try_recv().is_err(),
            "second event must not be queued live"
        );

        let replayed = registry.replay_all(&id);
        assert_eq!(replayed.len(), 2);
        assert_eq!(replayed[0].0, 0);
        assert_eq!(replayed[1].0, 1);
    }

    #[test]
    fn it_evicts_stale_disconnected_sessions() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (tx, _rx) = mpsc::channel(8);
        let generation = registry.register(id, tx);
        registry.unregister(&id, generation);

        {
            let session = registry.sessions.get_mut(&id).unwrap();
            *session
                .last_activity
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Instant::now() - Duration::from_secs(10);
        }

        registry.evict_stale(Duration::from_secs(1));
        assert!(registry.replay_all(&id).is_empty());
    }

    #[test]
    fn it_keeps_live_sessions_even_when_idle() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (tx, _rx) = mpsc::channel(8);
        registry.register(id, tx);

        {
            let session = registry.sessions.get_mut(&id).unwrap();
            *session
                .last_activity
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Instant::now() - Duration::from_secs(10);
        }

        registry.evict_stale(Duration::from_secs(1));
        assert!(registry.sessions.contains_key(&id));
    }
}

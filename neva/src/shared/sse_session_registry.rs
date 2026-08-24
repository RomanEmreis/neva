//! Per-stream SSE registry for server-side Last-Event-ID replay
//!
//! A session may hold more than one SSE stream at a time -- the spec lets a
//! client "remain connected to multiple SSE streams simultaneously" -- so the
//! cursor and the replay buffer belong to a *stream*, not to the session:
//! event ids are "assigned by servers on a per-stream basis, to act as a
//! cursor within that particular stream", and a resumption "MUST NOT replay
//! messages that would have been delivered on a different stream".
//!
//! One stream per session is its *standalone* one: the stream the server puts
//! its own traffic on, the messages that are "unrelated to any concurrently
//! running JSON-RPC request". It exists from the moment the session is minted
//! ([`SseSessionRegistry::pre_register`]), before any `GET` has arrived, so
//! that what the server emits during the `POST` -> `GET` window is buffered
//! against the stream that will carry it.

use crate::{
    error::{Error, ErrorCode},
    transport::http::core::types::EventId,
    types::Message,
};
use dashmap::DashMap;
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc::{Sender, error::TrySendError};
use uuid::Uuid;

/// Live channel of one SSE stream: the event's id and the message it carries.
pub(crate) type SseSender = Sender<(EventId, Arc<Message>)>;

/// Ephemeral channel of one SSE stream, for events that carry no id and are
/// never replayed -- `notifications/message`, written by the tracing layer.
#[cfg(feature = "tracing")]
pub(crate) type LogSender = Sender<Message>;

/// Most streams one session may hold at once.
///
/// Streams outlive the connection that opened them -- a disconnected one keeps
/// its buffer so the client can come back for it -- so without a cap a session
/// would accumulate one entry per `GET` it ever made. The cap is spent on live
/// streams first: a disconnected one is dropped to make room before a `GET` is
/// refused.
pub(crate) const MAX_STREAMS_PER_SESSION: usize = 8;

/// Bounded in-memory registry providing SSE event buffering and replay for
/// server-side Last-Event-ID support.
pub(crate) struct SseSessionRegistry {
    capacity: usize,
    sessions: DashMap<Uuid, SseSession>,
    next_gen: AtomicU64,
}

/// One session: the streams it holds, and which of them the server's own
/// traffic goes on.
struct SseSession {
    streams: HashMap<u64, SseStream>,
    /// The standalone stream -- where [`SseSessionRegistry::send`] delivers.
    /// Always a key of `streams`.
    standalone: u64,
    /// Next stream id to hand out. Ids are per session and only ever grow, so
    /// the lowest id is also the oldest stream.
    next_stream: u64,
    last_activity: Mutex<Instant>,
}

/// One SSE stream: its live channel, its cursor, and its replay buffer.
struct SseStream {
    sender: SseSender,
    /// Kept whether or not this stream currently carries the log channel: the
    /// standalone role moves between streams, and the one it moves to has to
    /// have a sender left to hand over.
    #[cfg(feature = "tracing")]
    log: LogSender,
    buffer: VecDeque<(u64, Arc<Message>)>,
    /// Cursor within this stream, and this stream alone.
    next_seq: u64,
    /// Updated on each reconnect to this stream, so a cleanup that belongs to
    /// the connection before it is a no-op.
    generation: u64,
}

/// A parsed `Last-Event-ID`.
struct Resume {
    /// The stream the id names. `None` for a bare `<seq>` -- the shape neva
    /// issued while a session had exactly one stream, and still accepted so a
    /// client reconnecting across a server upgrade resumes rather than
    /// restarts.
    stream: Option<u64>,
    seq: u64,
}

/// What the registry made of an SSE `GET`.
pub(crate) enum StreamSlot {
    /// The connection owns a stream. Its `replay` goes out before live events.
    Open(OpenStream),
    /// `Last-Event-ID` named a stream this session does not hold. Nothing here
    /// can be resumed, and no other stream may answer for it.
    UnknownStream,
    /// Every stream this session may hold is live. Refused rather than served,
    /// so the caller learns what happened instead of watching an idle stream.
    AtCapacity,
}

/// A stream a `GET` may write until its connection ends.
pub(crate) struct OpenStream {
    /// Id of the stream within its session -- the first half of every
    /// [`EventId`] it emits.
    pub(crate) stream: u64,
    /// Pass back to [`SseSessionRegistry::unregister`] when the connection
    /// ends.
    pub(crate) generation: u64,
    /// Buffered events this connection is owed before live ones.
    pub(crate) replay: Vec<(EventId, Arc<Message>)>,
}

impl Resume {
    /// Parses a `Last-Event-ID` value. `None` when it is not an id this
    /// server could have issued.
    fn parse(raw: &str) -> Option<Self> {
        match raw.split_once(':') {
            Some((stream, seq)) => Some(Self {
                stream: Some(stream.parse().ok()?),
                seq: seq.parse().ok()?,
            }),
            None => Some(Self {
                stream: None,
                seq: raw.parse().ok()?,
            }),
        }
    }
}

impl SseStream {
    fn new(sender: SseSender, #[cfg(feature = "tracing")] log: LogSender, generation: u64) -> Self {
        Self {
            sender,
            #[cfg(feature = "tracing")]
            log,
            buffer: VecDeque::new(),
            next_seq: 0,
            generation,
        }
    }

    /// Lets go of both of this stream's channels.
    ///
    /// Both, because the SSE response is the two of them merged and ends only
    /// when each has: a stream dropped for lagging has to reach the client as
    /// an EOF it can reconnect from, and holding on to the log sender -- which
    /// this registry does, so the standalone role can hand it over -- would
    /// leave that response open forever, carrying nothing. Only this stream's;
    /// the senders of the streams that may inherit the role are untouched.
    fn disconnect(&mut self) {
        self.sender = SseSessionRegistry::disconnected_sender();
        #[cfg(feature = "tracing")]
        {
            self.log = SseSessionRegistry::disconnected_log_sender();
        }
    }

    /// Buffered events with `seq > last_seq`, for a resumption of this stream.
    ///
    /// If `last_seq` was evicted (oldest buffered seq > `last_seq`), the full
    /// buffer is returned (best-effort replay).
    fn replay_since(&self, stream: u64, last_seq: u64) -> Vec<(EventId, Arc<Message>)> {
        let evicted = self.buffer.front().is_some_and(|(s, _)| *s > last_seq);
        self.buffer
            .iter()
            .filter(|(s, _)| evicted || *s > last_seq)
            .map(|(s, arc)| (EventId::new(stream, *s), arc.clone()))
            .collect()
    }

    /// Everything this stream still holds, in sequence order.
    fn replay_all(&self, stream: u64) -> Vec<(EventId, Arc<Message>)> {
        self.buffer
            .iter()
            .map(|(s, arc)| (EventId::new(stream, *s), arc.clone()))
            .collect()
    }
}

impl SseSession {
    /// A session with its standalone stream already in place, disconnected.
    fn new() -> Self {
        let mut streams = HashMap::with_capacity(1);
        streams.insert(
            0,
            SseStream::new(
                SseSessionRegistry::disconnected_sender(),
                #[cfg(feature = "tracing")]
                SseSessionRegistry::disconnected_log_sender(),
                0,
            ),
        );

        Self {
            streams,
            standalone: 0,
            next_stream: 1,
            last_activity: Mutex::new(Instant::now()),
        }
    }

    fn touch(&self) {
        *self.last_activity.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }

    fn idle_since(&self) -> Instant {
        *self.last_activity.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether a connection is currently reading `stream`.
    fn is_stream_live(&self, stream: u64) -> bool {
        self.streams
            .get(&stream)
            .is_some_and(|s| !s.sender.is_closed())
    }

    /// Whether any connection is reading any of this session's streams.
    fn has_live_stream(&self) -> bool {
        self.streams.values().any(|s| !s.sender.is_closed())
    }

    /// Moves the standalone role onto a live stream when the one holding it
    /// has gone away.
    ///
    /// Server-initiated traffic follows the role, so a session that still has a
    /// connection reading it goes on being served instead of piling events
    /// against a stream nobody is on. Newest first, for the reason a fresh
    /// `GET` takes the role in [`SseSessionRegistry::open`]: it is the
    /// connection the client is most certainly still reading.
    ///
    /// When nothing else is live the role stays where it is. That is the
    /// ordinary single-stream drop, and leaving it there is what lets the
    /// client's reconnect take that stream back and be replayed what it missed.
    ///
    /// Every stream a session holds today comes from a `GET` and is one the
    /// client is reading, so any of them may carry this. A request-scoped
    /// `POST` stream would not be, and would have to be kept out of here.
    ///
    /// Returns whether the role moved, which is the caller's cue to point the
    /// session's log channel at the stream it moved to
    /// ([`SseSessionRegistry::rebind_log`]).
    fn promote_standalone(&mut self) -> bool {
        if self.is_stream_live(self.standalone) {
            return false;
        }

        let live = self
            .streams
            .iter()
            .filter(|(_, stream)| !stream.sender.is_closed())
            .map(|(id, _)| *id)
            .max();

        match live {
            Some(id) => {
                self.standalone = id;
                true
            }
            None => false,
        }
    }

    /// Frees a slot when the session is at [`MAX_STREAMS_PER_SESSION`], by
    /// dropping the oldest stream nothing is connected to.
    ///
    /// The standalone stream is never the one dropped: it holds whatever the
    /// server buffered while nobody was connected, and its id is what a
    /// reconnect names.
    ///
    /// `false` when every stream is live and none may be dropped.
    fn make_room(&mut self) -> bool {
        if self.streams.len() < MAX_STREAMS_PER_SESSION {
            return true;
        }

        let standalone = self.standalone;
        let oldest = self
            .streams
            .iter()
            .filter(|(id, stream)| **id != standalone && stream.sender.is_closed())
            .map(|(id, _)| *id)
            .min();

        match oldest {
            Some(id) => {
                self.streams.remove(&id);
                true
            }
            None => false,
        }
    }
}

impl SseSessionRegistry {
    fn disconnected_sender() -> SseSender {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(rx);
        tx
    }

    #[cfg(feature = "tracing")]
    fn disconnected_log_sender() -> LogSender {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(rx);
        tx
    }

    /// Points the session's log channel at whichever stream holds the
    /// standalone role.
    ///
    /// Log events are ephemeral -- no id, never replayed -- so they ride the
    /// stream server-initiated traffic is on, and the role moves. `register`
    /// replaces whatever entry was there, and stamps the promoted stream's own
    /// generation, so that connection's cleanup still takes the right one down.
    #[cfg(feature = "tracing")]
    fn rebind_log(session_id: Uuid, session: &SseSession) {
        if let Some(stream) = session.streams.get(&session.standalone) {
            crate::types::notification::fmt::LOG_REGISTRY.register(
                session_id,
                stream.generation,
                stream.log.clone(),
            );
        }
    }

    /// Creates a new [`SseSessionRegistry`].
    ///
    /// `capacity` is the maximum number of events buffered per stream.
    /// `0` disables buffering (events still flow live; a resumption replays
    /// nothing).
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

    /// Hands an SSE `GET` the stream it asked for.
    ///
    /// `last_event_id` is the request's `Last-Event-ID` header, verbatim.
    ///
    /// * **With one**, the `GET` is resuming the stream that id names, and the
    ///   registry re-points that stream at the new connection and replays what
    ///   it buffered past the cursor. A live sender on that stream is replaced,
    ///   not refused: an id only ever reaches a client on the connection that
    ///   was handed it, so this is that stream coming back -- a client whose
    ///   TCP died before the server noticed, most often -- and not a second
    ///   stream. An id naming a stream the session does not hold is
    ///   [`StreamSlot::UnknownStream`]; nothing else may answer for it, and no
    ///   session is created to say so. An id in the pre-per-stream shape
    ///   (`<seq>`, naming no stream) resumes the standalone stream, but only
    ///   while the session has just the one -- which is the only way such an
    ///   id can have been issued.
    /// * **Without one**, the `GET` wants the standalone stream. It takes that
    ///   stream over when nothing is connected to it, which is what makes a
    ///   plain reconnect resume the buffer (and what carries the events emitted
    ///   during the `POST` -> `GET` window). When the standalone stream *is*
    ///   live, the `GET` gets a stream of its own -- the spec allows the second
    ///   connection, and the first is left open rather than displaced -- and
    ///   server-initiated traffic moves onto it, the newest standalone stream
    ///   being the one a client is certain to still be reading.
    ///
    /// The session is created if it does not exist.
    pub(crate) fn open(
        &self,
        id: Uuid,
        sender: SseSender,
        #[cfg(feature = "tracing")] log: LogSender,
        last_event_id: Option<&str>,
    ) -> StreamSlot {
        let resume = match last_event_id {
            // An id this server could not have issued names no stream, which
            // is the same answer as one naming a stream that is gone.
            Some(raw) => match Resume::parse(raw) {
                Some(resume) => Some(resume),
                None => return StreamSlot::UnknownStream,
            },
            None => None,
        };

        // A cursor naming a stream is judged against the session that would
        // hold it, and judged before `entry` below can create one: a `GET`
        // about to be refused must leave nothing behind, or every refusal
        // costs a session entry until the sweep gets to it.
        if let Some(Resume {
            stream: Some(target),
            ..
        }) = &resume
        {
            let known = self
                .sessions
                .get(&id)
                .is_some_and(|session| session.streams.contains_key(target));

            if !known {
                return StreamSlot::UnknownStream;
            }
        }

        let generation = self.next_gen.fetch_add(1, Ordering::Relaxed) + 1;
        let mut session = self.sessions.entry(id).or_insert_with(SseSession::new);
        session.touch();

        let target = match &resume {
            Some(Resume {
                stream: Some(stream),
                ..
            }) => {
                if !session.streams.contains_key(stream) {
                    return StreamSlot::UnknownStream;
                }
                *stream
            }
            // A bare `<seq>` is an id from before they named a stream, issued
            // by a server that had one stream per session to be counting. Read
            // against a session that has since opened more, it would replay one
            // stream's backlog under a count that was never its own -- so it is
            // honoured only where it could have been issued, and refused
            // otherwise rather than answered with the wrong stream.
            Some(Resume { stream: None, .. }) => {
                if session.streams.len() != 1 {
                    return StreamSlot::UnknownStream;
                }

                session.standalone
            }
            None if !session.is_stream_live(session.standalone) => session.standalone,
            // Standalone taken, and this `GET` is not resuming: a new stream,
            // and the one server traffic moves onto.
            None => {
                if !session.make_room() {
                    return StreamSlot::AtCapacity;
                }
                let stream = session.next_stream;
                session.next_stream += 1;
                session.streams.insert(
                    stream,
                    SseStream::new(
                        sender,
                        #[cfg(feature = "tracing")]
                        log,
                        generation,
                    ),
                );

                session.standalone = stream;
                #[cfg(feature = "tracing")]
                Self::rebind_log(id, &session);

                return StreamSlot::Open(OpenStream {
                    stream,
                    generation,
                    replay: Vec::new(),
                });
            }
        };

        // `target` was either checked above or is the standalone stream, which
        // a session always holds -- the arm is what the lookup returns rather
        // than a case that arises.
        let Some(stream) = session.streams.get_mut(&target) else {
            return StreamSlot::UnknownStream;
        };

        stream.sender = sender;
        #[cfg(feature = "tracing")]
        {
            stream.log = log;
        }
        stream.generation = generation;
        let replay = match &resume {
            Some(resume) => stream.replay_since(target, resume.seq),
            None => stream.replay_all(target),
        };

        // A resumption is a connection again, and the role may be sitting on a
        // stream nobody is reading -- both went away, and this is the one that
        // came back. Moving it here rather than leaving `send` to discover it
        // matters: `send` only promotes *after* a failed delivery, so the first
        // message would have been numbered against the dead stream and left in
        // its buffer, or dropped outright with buffering off.
        session.promote_standalone();
        // Unconditional, not only on a promotion: the role may have been on
        // this stream all along, and this is a new connection to it.
        #[cfg(feature = "tracing")]
        Self::rebind_log(id, &session);

        StreamSlot::Open(OpenStream {
            stream: target,
            generation,
            replay,
        })
    }

    /// Disconnects one stream, only if its stored generation matches
    /// `generation`.
    ///
    /// No-op when the stream has been re-opened by a newer connection,
    /// preventing stale cleanup from disconnecting a live one. Buffered replay
    /// state is preserved so clients can resume after a transient drop.
    pub(crate) fn unregister(&self, id: &Uuid, stream: u64, generation: u64) {
        let Some(mut session) = self.sessions.get_mut(id) else {
            return;
        };

        let disconnected = match session.streams.get_mut(&stream) {
            Some(stream) if stream.generation == generation => {
                stream.disconnect();
                true
            }
            _ => false,
        };

        if disconnected {
            session.touch();
            // The connection that just ended may have been the one carrying
            // server-initiated traffic; another may still be open to carry it.
            // The log channel goes with it -- this stream's entry was removed
            // by the same cleanup that called here.
            let _promoted = session.promote_standalone();
            #[cfg(feature = "tracing")]
            if _promoted {
                Self::rebind_log(*id, &session);
            }
        }
    }

    /// Unconditionally removes a session and every stream it holds.
    ///
    /// Use for explicit session termination (e.g. DELETE /mcp). Unlike
    /// [`unregister`](Self::unregister), this does not check the generation --
    /// the session is always removed.
    pub(crate) fn terminate(&self, id: &Uuid) {
        self.sessions.remove(id);
    }

    /// Buffers `message` against the session's standalone stream and sends it
    /// to that stream's live channel.
    ///
    /// The standalone stream is where server-initiated traffic belongs, and
    /// naming one stream is also what keeps the server from broadcasting the
    /// same message across every stream a session holds, which the spec
    /// forbids.
    ///
    /// Buffer-first: the message is stored before the channel send, so a dead
    /// channel does not lose the event -- it remains available for the next
    /// reconnect. If the session is not found, the event is dropped (no buffer
    /// to write to).
    pub(crate) fn send(&self, message: Message) -> Result<(), Error> {
        let Some(&session_id) = message.session_id() else {
            return Err(Error::new(ErrorCode::InvalidParams, "missing session id"));
        };

        let Some(mut session) = self.sessions.get_mut(&session_id) else {
            #[cfg(feature = "tracing")]
            tracing::warn!(
                logger = "neva",
                "Session {} not found for SSE send -- event dropped",
                session_id
            );
            return Ok(());
        };

        session.touch();
        let capacity = self.capacity;
        let stream_id = session.standalone;
        // A session always holds its standalone stream; nothing removes it.
        let Some(stream) = session.streams.get_mut(&stream_id) else {
            return Ok(());
        };

        let arc = Arc::new(message);
        let seq = stream.next_seq;
        stream.next_seq += 1;

        if capacity > 0 {
            stream.buffer.push_back((seq, arc.clone()));
            while stream.buffer.len() > capacity {
                stream.buffer.pop_front();
            }
        }

        let event = EventId::new(stream_id, seq);
        let _generation = stream.generation;
        let mut dropped = false;
        match stream.sender.try_send((event, arc)) {
            Ok(()) => {}
            Err(TrySendError::Full((_event, _arc))) => {
                stream.disconnect();
                dropped = true;
                #[cfg(feature = "tracing")]
                {
                    // Only this stream's log channel: a newer standalone stream
                    // has a registration of its own, and lagging behind on the
                    // one before it is no reason to take that one down.
                    crate::types::notification::fmt::LOG_REGISTRY
                        .unregister_if_generation(&session_id, _generation);
                    tracing::warn!(
                        logger = "neva",
                        "Lagging SSE client for session {}: disconnecting SSE stream at {}",
                        session_id,
                        event
                    );
                }
            }
            Err(TrySendError::Closed((_event, _arc))) => {
                stream.disconnect();
                dropped = true;
                #[cfg(feature = "tracing")]
                {
                    crate::types::notification::fmt::LOG_REGISTRY
                        .unregister_if_generation(&session_id, _generation);
                    tracing::warn!(
                        logger = "neva",
                        "Dead channel for session {}: {} is in buffer for next reconnect",
                        session_id,
                        event
                    );
                }
            }
        }

        // This event keeps the id it was given -- it is in that stream's buffer
        // under it, waiting for a resumption. What comes next goes wherever the
        // role has moved to.
        if dropped {
            let _promoted = session.promote_standalone();
            #[cfg(feature = "tracing")]
            if _promoted {
                Self::rebind_log(session_id, &session);
            }
        }

        Ok(())
    }

    /// Creates a session entry, with its standalone stream, if one does not
    /// already exist.
    ///
    /// Call when a session ID is first minted (on POST /mcp) so that any
    /// server-initiated events emitted before the client's SSE GET arrive are
    /// buffered and available for replay. If an entry already exists (live
    /// connection or prior pre-registration), this is a no-op -- the existing
    /// streams are preserved.
    ///
    /// The entry is created even with buffering disabled (`capacity == 0`): it
    /// is also what makes the session *known*, and a server that buffers
    /// nothing still has to tell a live session from a terminated one.
    /// Buffering stays governed by the registry's capacity.
    // Stateless 2026-07-28 transport skips pre-registration (no SSE GET); the method
    // stays compiled for the legacy build.
    #[cfg_attr(not(feature = "legacy-spec"), allow(dead_code))]
    pub(crate) fn pre_register(&self, id: Uuid) {
        self.sessions.entry(id).or_insert_with(SseSession::new);
    }

    /// Reports whether `id` names a session this server still holds, counting
    /// the visit as activity.
    ///
    /// The refresh is what keeps a session that only ever POSTs alive: no
    /// stream of its is connected, so without a touch on each request
    /// [`evict_stale`](Self::evict_stale) would reap it mid-conversation and
    /// the next request would be answered as a terminated session.
    #[cfg_attr(not(feature = "legacy-spec"), allow(dead_code))]
    pub(crate) fn is_live(&self, id: &Uuid) -> bool {
        match self.sessions.get(id) {
            Some(session) => {
                session.touch();
                true
            }
            None => false,
        }
    }

    /// Removes sessions with no connected stream whose last activity is older
    /// than `ttl`.
    pub(crate) fn evict_stale(&self, ttl: Duration) {
        let now = Instant::now();
        let stale = |session: &SseSession| {
            !session.has_live_stream() && now.saturating_duration_since(session.idle_since()) >= ttl
        };

        let stale_ids: Vec<Uuid> = self
            .sessions
            .iter()
            .filter_map(|entry| stale(entry.value()).then_some(*entry.key()))
            .collect();

        for id in stale_ids {
            let _removed = self.sessions.remove_if(&id, |_, session| stale(session));
            #[cfg(feature = "tracing")]
            if _removed.is_some() {
                crate::types::notification::fmt::LOG_REGISTRY.unregister(&id);
            }
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
impl SseSessionRegistry {
    /// Sequence numbers `stream` still holds, oldest first.
    fn buffered(&self, id: &Uuid, stream: u64) -> Vec<u64> {
        self.sessions
            .get(id)
            .and_then(|session| {
                session
                    .streams
                    .get(&stream)
                    .map(|s| s.buffer.iter().map(|(seq, _)| *seq).collect())
            })
            .unwrap_or_default()
    }

    /// How many streams the session holds.
    fn stream_count(&self, id: &Uuid) -> usize {
        self.sessions.get(id).map_or(0, |s| s.streams.len())
    }

    /// The stream server-initiated traffic currently goes to.
    fn standalone(&self, id: &Uuid) -> Option<u64> {
        self.sessions.get(id).map(|s| s.standalone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::notification::Notification;
    use tokio::sync::mpsc::{self, Receiver};

    fn make_msg(session_id: Uuid) -> Message {
        Message::Notification(Notification::new("test", None)).set_session_id(session_id)
    }

    /// Opens a stream the way a `GET` would, panicking on anything but a
    /// stream: the tests that are about a refusal match on it themselves.
    fn open(
        registry: &SseSessionRegistry,
        id: Uuid,
        last_event_id: Option<&str>,
        queue: usize,
    ) -> (OpenStream, Receiver<(EventId, Arc<Message>)>) {
        let (tx, rx) = mpsc::channel(queue);
        // The log half is only interesting to the tests that follow it; the
        // rest let the receiver go, which is what a stream carrying no logs
        // looks like anyway.
        #[cfg(feature = "tracing")]
        let (log_tx, _log_rx) = mpsc::channel(queue);
        match registry.open(
            id,
            tx,
            #[cfg(feature = "tracing")]
            log_tx,
            last_event_id,
        ) {
            StreamSlot::Open(open) => (open, rx),
            StreamSlot::UnknownStream => panic!("the registry refused a known stream"),
            StreamSlot::AtCapacity => panic!("the registry refused a stream it had room for"),
        }
    }

    /// A log channel nothing reads -- the refusal tests never get far enough
    /// for one to matter.
    #[cfg(feature = "tracing")]
    fn log_sender() -> LogSender {
        let (tx, _rx) = mpsc::channel(1);
        tx
    }

    fn ids(rx: &mut Receiver<(EventId, Arc<Message>)>) -> Vec<EventId> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .map(|(id, _)| id)
            .collect()
    }

    #[test]
    fn it_renders_an_event_id_as_stream_and_seq() {
        assert_eq!(EventId::new(2, 7).to_string(), "2:7");
    }

    #[test]
    fn it_parses_a_stream_qualified_last_event_id() {
        let resume = Resume::parse("3:11").expect("a well-formed id");
        assert_eq!(resume.stream, Some(3));
        assert_eq!(resume.seq, 11);
    }

    #[test]
    fn it_parses_a_bare_last_event_id_as_naming_no_stream() {
        let resume = Resume::parse("11").expect("the pre-per-stream id shape");
        assert_eq!(resume.stream, None);
        assert_eq!(resume.seq, 11);
    }

    #[test]
    fn it_rejects_a_last_event_id_it_could_not_have_issued() {
        assert!(Resume::parse("nonsense").is_none());
        assert!(Resume::parse("1:").is_none());
        assert!(Resume::parse("a:1").is_none());
        assert!(Resume::parse("1:2:3").is_none());
    }

    #[test]
    fn it_returns_generation_1_for_first_registration() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (open, _rx) = open(&registry, id, None, 8);
        assert_eq!(open.generation, 1);
    }

    #[test]
    fn it_returns_higher_generation_on_reconnect() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (first, rx1) = open(&registry, id, None, 8);
        drop(rx1);
        let (second, _rx2) = open(&registry, id, None, 8);
        assert!(
            second.generation > first.generation,
            "second connection must have a higher generation"
        );
        assert_eq!(
            second.stream, first.stream,
            "a reconnect resumes the standalone stream rather than opening one"
        );
    }

    #[test]
    fn it_preserves_buffer_and_cursor_on_reconnect() {
        let registry = SseSessionRegistry::new(16);
        let id = Uuid::new_v4();

        // First connection: send 3 events -> seqs 0, 1, 2
        let (first, rx1) = open(&registry, id, None, 16);
        for _ in 0..3 {
            registry.send(make_msg(id)).unwrap();
        }
        drop(rx1);

        // Reconnect. Must NOT reset the cursor.
        let (second, mut rx2) = open(&registry, id, None, 16);
        assert_eq!(second.replay.len(), 3, "the buffer is replayed in full");

        registry.send(make_msg(id)).unwrap();
        registry.send(make_msg(id)).unwrap();

        assert_eq!(
            ids(&mut rx2),
            vec![EventId::new(first.stream, 3), EventId::new(first.stream, 4)],
            "seq must continue after a reconnect, not reset to 0"
        );
    }

    #[test]
    fn it_replays_only_what_the_cursor_missed() {
        let registry = SseSessionRegistry::new(16);
        let id = Uuid::new_v4();
        let (first, rx1) = open(&registry, id, None, 16);
        for _ in 0..5 {
            registry.send(make_msg(id)).unwrap();
        }
        drop(rx1);

        let resume = EventId::new(first.stream, 2).to_string();
        let (second, _rx2) = open(&registry, id, Some(&resume), 16);
        assert_eq!(
            second.replay.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![EventId::new(first.stream, 3), EventId::new(first.stream, 4)]
        );
    }

    #[test]
    fn it_replays_the_full_buffer_when_the_cursor_was_evicted() {
        let registry = SseSessionRegistry::new(3);
        let id = Uuid::new_v4();
        let (first, rx1) = open(&registry, id, None, 8);
        for _ in 0..5 {
            registry.send(make_msg(id)).unwrap();
        }
        drop(rx1);

        // Buffer holds seqs 2, 3, 4; the client asks from 0, which is gone.
        let resume = EventId::new(first.stream, 0).to_string();
        let (second, _rx2) = open(&registry, id, Some(&resume), 8);
        assert_eq!(second.replay.len(), 3);
        assert_eq!(second.replay[0].0, EventId::new(first.stream, 2));
    }

    #[test]
    fn it_replays_nothing_when_the_cursor_is_the_newest_event() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (first, rx1) = open(&registry, id, None, 8);
        for _ in 0..3 {
            registry.send(make_msg(id)).unwrap();
        }
        drop(rx1);

        let resume = EventId::new(first.stream, 2).to_string();
        let (second, _rx2) = open(&registry, id, Some(&resume), 8);
        assert!(second.replay.is_empty());
    }

    #[test]
    fn it_resumes_the_standalone_stream_for_an_id_that_names_no_stream() {
        // What a client that last spoke to a neva old enough to number events
        // per session sends back after the server is upgraded under it.
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (first, rx1) = open(&registry, id, None, 8);
        for _ in 0..3 {
            registry.send(make_msg(id)).unwrap();
        }
        drop(rx1);

        let (second, _rx2) = open(&registry, id, Some("1"), 8);
        assert_eq!(second.stream, first.stream);
        assert_eq!(registry.standalone(&id), Some(second.stream));
        assert_eq!(
            second.replay.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![EventId::new(first.stream, 2)],
            "the bare cursor is read against the stream it must have come from"
        );
    }

    #[test]
    fn it_refuses_an_id_naming_a_stream_the_session_does_not_hold() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        registry.pre_register(id);

        let (tx, _rx) = mpsc::channel(8);
        assert!(matches!(
            registry.open(
                id,
                tx,
                #[cfg(feature = "tracing")]
                log_sender(),
                Some("7:1")
            ),
            StreamSlot::UnknownStream
        ));
        assert_eq!(
            registry.stream_count(&id),
            1,
            "a refusal must not leave a stream behind"
        );
    }

    #[test]
    fn it_does_not_create_a_session_to_refuse_a_cursor() {
        // A refusal that left a session behind would cost one per `GET` that
        // got this far, held until the stale sweep.
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();

        let (tx, _rx) = mpsc::channel(8);
        assert!(matches!(
            registry.open(
                id,
                tx,
                #[cfg(feature = "tracing")]
                log_sender(),
                Some("3:1")
            ),
            StreamSlot::UnknownStream
        ));
        assert!(!registry.sessions.contains_key(&id));
    }

    #[test]
    fn it_refuses_a_bare_cursor_once_the_session_holds_more_than_one_stream() {
        // The shape can only have come from a server that numbered a session's
        // one stream, so against several it names nothing -- and answering it
        // from the standalone stream would replay a backlog under a count that
        // was never that stream's.
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();

        let (_first, _rx1) = open(&registry, id, None, 8);
        let (_second, _rx2) = open(&registry, id, None, 8);

        let (tx, _rx) = mpsc::channel(8);
        assert!(matches!(
            registry.open(
                id,
                tx,
                #[cfg(feature = "tracing")]
                log_sender(),
                Some("1")
            ),
            StreamSlot::UnknownStream
        ));
    }

    #[test]
    fn it_refuses_a_last_event_id_it_cannot_read() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        registry.pre_register(id);

        let (tx, _rx) = mpsc::channel(8);
        assert!(matches!(
            registry.open(
                id,
                tx,
                #[cfg(feature = "tracing")]
                log_sender(),
                Some("not-an-id")
            ),
            StreamSlot::UnknownStream
        ));
    }

    #[test]
    fn it_opens_a_second_stream_without_closing_the_first() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();

        let (first, mut rx1) = open(&registry, id, None, 8);
        let (second, mut rx2) = open(&registry, id, None, 8);

        assert_ne!(
            second.stream, first.stream,
            "a concurrent GET gets a stream of its own"
        );
        assert_eq!(registry.stream_count(&id), 2);
        assert!(
            !rx1.is_closed(),
            "the first stream must be left open, not displaced"
        );

        // Server-initiated traffic goes on exactly one of them -- the newest.
        registry.send(make_msg(id)).unwrap();
        assert_eq!(ids(&mut rx2), vec![EventId::new(second.stream, 0)]);
        assert!(
            rx1.try_recv().is_err(),
            "the same message must not be broadcast across both streams"
        );
    }

    #[test]
    fn it_resumes_one_stream_without_replaying_another() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();

        // Stream A collects two events, then the traffic moves to stream B.
        let (a, rx_a) = open(&registry, id, None, 8);
        registry.send(make_msg(id)).unwrap();
        registry.send(make_msg(id)).unwrap();
        let (b, _rx_b) = open(&registry, id, None, 8);
        registry.send(make_msg(id)).unwrap();
        drop(rx_a);

        // A resumption of A replays A's events and no others.
        let resume = EventId::new(a.stream, 0).to_string();
        let (resumed, _rx) = open(&registry, id, Some(&resume), 8);
        assert_eq!(resumed.stream, a.stream);
        assert_eq!(
            resumed.replay.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![EventId::new(a.stream, 1)],
        );
        assert_eq!(
            registry.standalone(&id),
            Some(b.stream),
            "resuming an older stream does not take the route back"
        );
    }

    #[test]
    fn it_takes_over_a_stream_that_is_resumed_while_still_live() {
        // The client's connection died without the server noticing; the id it
        // comes back with belongs to that same stream.
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (first, mut rx1) = open(&registry, id, None, 8);
        registry.send(make_msg(id)).unwrap();
        assert!(rx1.try_recv().is_ok());

        let resume = EventId::new(first.stream, 0).to_string();
        let (second, mut rx2) = open(&registry, id, Some(&resume), 8);
        assert_eq!(
            second.stream, first.stream,
            "the same stream coming back is not a second one"
        );
        assert_eq!(registry.stream_count(&id), 1);

        registry.send(make_msg(id)).unwrap();
        assert_eq!(ids(&mut rx2), vec![EventId::new(first.stream, 1)]);
        assert!(rx1.try_recv().is_err(), "the dead connection gets nothing");
    }

    #[test]
    fn it_moves_server_traffic_to_a_live_stream_when_the_standalone_one_drops() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();

        // Two connections; the newer one carries the traffic.
        let (first, mut rx1) = open(&registry, id, None, 8);
        let (second, rx2) = open(&registry, id, None, 8);
        assert_eq!(registry.standalone(&id), Some(second.stream));

        // The newer connection ends while the older one is still reading.
        drop(rx2);
        registry.unregister(&id, second.stream, second.generation);

        registry.send(make_msg(id)).unwrap();
        assert_eq!(
            ids(&mut rx1),
            vec![EventId::new(first.stream, 0)],
            "the surviving connection carries the session's traffic"
        );
        assert_eq!(registry.standalone(&id), Some(first.stream));
    }

    #[test]
    fn it_moves_server_traffic_to_a_resumed_stream_when_the_standalone_one_is_dead() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();

        let (first, rx1) = open(&registry, id, None, 8);
        let (second, rx2) = open(&registry, id, None, 8);

        // Both connections go away: there is nothing live to hand the role to,
        // so it stays on the newer stream, waiting for a reconnect.
        drop(rx1);
        registry.unregister(&id, first.stream, first.generation);
        drop(rx2);
        registry.unregister(&id, second.stream, second.generation);
        assert_eq!(registry.standalone(&id), Some(second.stream));

        // The client comes back for the older stream, by its id.
        let resume = EventId::new(first.stream, 0).to_string();
        let (resumed, mut rx) = open(&registry, id, Some(&resume), 8);
        assert_eq!(resumed.stream, first.stream);
        assert_eq!(
            registry.standalone(&id),
            Some(first.stream),
            "the one live stream is the one the traffic goes on"
        );

        registry.send(make_msg(id)).unwrap();
        assert_eq!(
            ids(&mut rx),
            vec![EventId::new(first.stream, 0)],
            "the first message after the resume reaches the connection"
        );
        assert!(
            registry.buffered(&id, second.stream).is_empty(),
            "and is not numbered against the stream nobody is on"
        );
    }

    /// Ephemeral log events ride the stream server-initiated traffic is on, so
    /// they have to move with it -- otherwise a session with a surviving
    /// connection keeps getting tracked events on it and drops every
    /// `notifications/message` until some later `GET`.
    #[cfg(feature = "tracing")]
    #[test]
    fn it_moves_the_log_channel_with_the_standalone_role() {
        use crate::types::notification::fmt::LOG_REGISTRY;

        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();

        let (tx1, _rx1) = mpsc::channel(8);
        let (log_tx1, mut log_rx1) = mpsc::channel(8);
        let StreamSlot::Open(first) = registry.open(id, tx1, log_tx1, None) else {
            panic!("the first GET was refused")
        };

        let (tx2, rx2) = mpsc::channel(8);
        let (log_tx2, mut log_rx2) = mpsc::channel(8);
        let StreamSlot::Open(second) = registry.open(id, tx2, log_tx2, None) else {
            panic!("the second GET was refused")
        };

        // The newest stream holds the role, so it holds the log channel.
        LOG_REGISTRY.send(make_msg(id)).unwrap();
        assert!(log_rx2.try_recv().is_ok());
        assert!(
            log_rx1.try_recv().is_err(),
            "a log event goes to one stream, like any other"
        );

        // That connection ends, the way the SSE cleanup guard ends one: the
        // log entry is taken down first, then the stream is disconnected.
        drop(rx2);
        LOG_REGISTRY.unregister_if_generation(&id, second.generation);
        registry.unregister(&id, second.stream, second.generation);

        assert_eq!(registry.standalone(&id), Some(first.stream));
        LOG_REGISTRY.send(make_msg(id)).unwrap();
        assert!(
            log_rx1.try_recv().is_ok(),
            "the promoted stream carries the logs too"
        );
    }

    #[test]
    fn it_leaves_the_standalone_slot_alone_when_nothing_else_is_live() {
        // The ordinary single-stream drop: events pile up against the stream
        // the client is coming back to, which is what makes the replay work.
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();

        let (only, rx) = open(&registry, id, None, 8);
        drop(rx);
        registry.unregister(&id, only.stream, only.generation);

        registry.send(make_msg(id)).unwrap();
        assert_eq!(registry.standalone(&id), Some(only.stream));
        assert_eq!(registry.buffered(&id, only.stream), vec![0]);

        let (resumed, _rx) = open(&registry, id, None, 8);
        assert_eq!(resumed.stream, only.stream);
        assert_eq!(resumed.replay.len(), 1, "the reconnect is replayed it");
    }

    #[test]
    fn it_moves_server_traffic_off_a_lagging_standalone_stream() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();

        let (first, mut rx1) = open(&registry, id, None, 8);
        // The newer connection's queue holds one event and is never drained.
        let (second, _rx2) = open(&registry, id, None, 1);

        registry.send(make_msg(id)).unwrap(); // fills the live queue
        registry.send(make_msg(id)).unwrap(); // declares it lagging

        // The event that found the queue full stays where it was numbered,
        // for a resumption of that stream; the next one goes to the survivor.
        assert_eq!(registry.buffered(&id, second.stream), vec![0, 1]);
        registry.send(make_msg(id)).unwrap();
        assert_eq!(ids(&mut rx1), vec![EventId::new(first.stream, 0)]);
    }

    #[test]
    fn it_drops_a_disconnected_stream_to_make_room() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();

        // Fill the session to its cap, dropping every connection but the last.
        let mut live = Vec::new();
        for _ in 0..MAX_STREAMS_PER_SESSION {
            let (_open, rx) = open(&registry, id, None, 8);
            live.push(rx);
        }
        assert_eq!(registry.stream_count(&id), MAX_STREAMS_PER_SESSION);
        // Disconnect the oldest non-standalone stream.
        live.remove(1);

        let (fresh, _rx) = open(&registry, id, None, 8);
        assert_eq!(
            registry.stream_count(&id),
            MAX_STREAMS_PER_SESSION,
            "the cap holds"
        );
        assert_eq!(registry.standalone(&id), Some(fresh.stream));
    }

    #[test]
    fn it_refuses_a_stream_when_every_slot_is_live() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();

        let mut live = Vec::new();
        for _ in 0..MAX_STREAMS_PER_SESSION {
            let (_open, rx) = open(&registry, id, None, 8);
            live.push(rx);
        }

        let (tx, _rx) = mpsc::channel(8);
        assert!(matches!(
            registry.open(
                id,
                tx,
                #[cfg(feature = "tracing")]
                log_sender(),
                None
            ),
            StreamSlot::AtCapacity
        ));
    }

    #[test]
    fn it_disconnects_a_stream_when_the_generation_matches() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (open, mut rx) = open(&registry, id, None, 8);
        registry.unregister(&id, open.stream, open.generation);

        registry.send(make_msg(id)).unwrap();
        assert!(rx.try_recv().is_err(), "live sender must be disconnected");
        assert_eq!(
            registry.buffered(&id, open.stream),
            vec![0],
            "buffer must be preserved"
        );
    }

    #[test]
    fn it_does_not_disconnect_a_stream_on_a_stale_generation() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();

        let (first, rx1) = open(&registry, id, None, 8);
        drop(rx1);
        let (second, mut rx2) = open(&registry, id, None, 8);

        // The previous connection's cleanup must be a no-op.
        registry.unregister(&id, first.stream, first.generation);

        registry.send(make_msg(id)).unwrap();
        assert!(rx2.try_recv().is_ok(), "the live stream must be preserved");
        assert_eq!(second.stream, first.stream);
    }

    #[test]
    fn it_terminates_a_session_unconditionally() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (open, _rx) = open(&registry, id, None, 8);
        registry.send(make_msg(id)).unwrap();
        registry.terminate(&id);
        assert_eq!(registry.stream_count(&id), 0);
        assert!(registry.buffered(&id, open.stream).is_empty());
    }

    #[test]
    fn it_buffers_an_event_and_delivers_it_to_the_channel() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (open, mut rx) = open(&registry, id, None, 8);

        registry.send(make_msg(id)).unwrap();
        let (event, _) = rx.try_recv().expect("event must be delivered live");
        assert_eq!(event, EventId::new(open.stream, 0));
        assert_eq!(registry.buffered(&id, open.stream), vec![0]);
    }

    #[test]
    fn it_shares_one_arc_between_buffer_and_channel() {
        let registry = SseSessionRegistry::new(1);
        let id = Uuid::new_v4();
        let (_open, mut rx) = open(&registry, id, None, 1);
        registry.send(make_msg(id)).unwrap();

        let (_, arc_live) = rx.try_recv().unwrap();
        assert_eq!(
            Arc::strong_count(&arc_live),
            2, // 1 from channel (arc_live) + 1 still in buffer
            "buffer and channel must share one Arc allocation"
        );
    }

    #[test]
    fn it_evicts_the_oldest_event_when_the_buffer_is_full() {
        let registry = SseSessionRegistry::new(3);
        let id = Uuid::new_v4();
        let (open, _rx) = open(&registry, id, None, 8);

        for _ in 0..4 {
            registry.send(make_msg(id)).unwrap();
        }
        assert_eq!(registry.buffered(&id, open.stream), vec![1, 2, 3]);
    }

    #[test]
    fn it_buffers_nothing_when_capacity_is_zero() {
        let registry = SseSessionRegistry::new(0);
        let id = Uuid::new_v4();
        let (open, mut rx) = open(&registry, id, None, 1);

        registry.send(make_msg(id)).unwrap();

        assert!(rx.try_recv().is_ok(), "the event is still delivered live");
        assert!(registry.buffered(&id, open.stream).is_empty());
    }

    #[test]
    fn it_keeps_buffering_when_the_channel_is_dead() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (open, rx) = open(&registry, id, None, 8);
        drop(rx); // kill the channel

        // send() must not fail -- the event is buffered for the next reconnect
        registry.send(make_msg(id)).unwrap();
        registry.send(make_msg(id)).unwrap();
        assert_eq!(registry.buffered(&id, open.stream), vec![0, 1]);
    }

    #[test]
    fn it_disconnects_a_live_stream_when_its_queue_fills() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (open, mut rx) = open(&registry, id, None, 1);

        registry.send(make_msg(id)).unwrap(); // fills the live queue with seq=0
        registry.send(make_msg(id)).unwrap(); // seq=1 disconnects it

        let (event, _) = rx.try_recv().expect("first event must remain queued");
        assert_eq!(event, EventId::new(open.stream, 0));
        assert!(
            rx.try_recv().is_err(),
            "second event must not be queued live"
        );
        assert_eq!(registry.buffered(&id, open.stream), vec![0, 1]);
    }

    #[test]
    fn it_buffers_events_during_the_pre_registration_window() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();

        // POST /initialize: session minted, SSE GET not yet arrived
        registry.pre_register(id);

        // Events sent before the SSE GET are buffered (dead channel, not an error)
        registry.send(make_msg(id)).unwrap(); // seq=0
        registry.send(make_msg(id)).unwrap(); // seq=1

        // GET /mcp: the standalone stream is taken over, buffer intact
        let (open, mut rx) = open(&registry, id, None, 8);
        assert_eq!(
            open.replay.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![EventId::new(open.stream, 0), EventId::new(open.stream, 1)]
        );

        registry.send(make_msg(id)).unwrap(); // seq=2
        let (event, _) = rx.try_recv().expect("seq=2 must be delivered live");
        assert_eq!(event, EventId::new(open.stream, 2));
    }

    #[test]
    fn it_pre_registers_without_disturbing_a_live_session() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();

        let (open, mut rx) = open(&registry, id, None, 8);
        registry.send(make_msg(id)).unwrap(); // seq=0

        registry.pre_register(id);

        registry.send(make_msg(id)).unwrap(); // seq=1
        assert_eq!(
            ids(&mut rx),
            vec![EventId::new(open.stream, 0), EventId::new(open.stream, 1)]
        );
    }

    #[test]
    fn it_pre_registers_a_known_session_even_with_buffering_off() {
        let registry = SseSessionRegistry::new(0);
        let id = Uuid::new_v4();
        registry.pre_register(id);
        assert!(
            registry.is_live(&id),
            "the session must be known even when nothing is buffered"
        );
        assert!(registry.buffered(&id, 0).is_empty());
    }

    #[test]
    fn it_evicts_stale_disconnected_sessions() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (open, _rx) = open(&registry, id, None, 8);
        registry.unregister(&id, open.stream, open.generation);

        if let Some(session) = registry.sessions.get(&id) {
            *session
                .last_activity
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Instant::now() - Duration::from_secs(10);
        }

        registry.evict_stale(Duration::from_secs(1));
        assert_eq!(registry.stream_count(&id), 0);
    }

    #[test]
    fn it_keeps_a_session_with_a_live_stream_even_when_idle() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (_open, _rx) = open(&registry, id, None, 8);

        if let Some(session) = registry.sessions.get(&id) {
            *session
                .last_activity
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Instant::now() - Duration::from_secs(10);
        }

        registry.evict_stale(Duration::from_secs(1));
        assert!(registry.sessions.contains_key(&id));
    }

    #[test]
    fn it_keeps_a_session_whose_second_stream_is_still_live() {
        let registry = SseSessionRegistry::new(8);
        let id = Uuid::new_v4();
        let (first, rx1) = open(&registry, id, None, 8);
        let (_second, _rx2) = open(&registry, id, None, 8);
        drop(rx1);
        registry.unregister(&id, first.stream, first.generation);

        if let Some(session) = registry.sessions.get(&id) {
            *session
                .last_activity
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Instant::now() - Duration::from_secs(10);
        }

        registry.evict_stale(Duration::from_secs(1));
        assert!(
            registry.sessions.contains_key(&id),
            "one dead stream does not make the session stale"
        );
    }
}

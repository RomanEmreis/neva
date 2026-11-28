//! stdio transport implementation

use crate::error::{Error, ErrorCode};
use crate::transport::{
    DrainGuard, DrainSignal, Receiver as TransportReceiver, Sender as TransportSender, Transport,
    TransportHandle,
};
use crate::types::Message;
use futures_util::TryFutureExt;
use tokio::sync::mpsc::{self, Receiver, Sender};

// The async reader and writer serve a child process's pipes, which only a
// client has (and the in-memory link, which needs one too). A server reads
// its own stdin and writes its own stdout, each on a thread of its own -- see
// `StdIoReceiver::start_blocking` and `StdIoSender::start_blocking`.
#[cfg(feature = "client")]
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

#[cfg(feature = "server")]
use std::{
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};

#[cfg(feature = "testing")]
use tokio::io::DuplexStream;

#[cfg(feature = "client")]
use self::options::StdIoOptions;
#[cfg(feature = "client")]
use tokio::process::{ChildStdin, ChildStdout};

#[cfg(all(feature = "client", target_os = "linux"))]
mod linux;
#[cfg(all(feature = "client", target_os = "windows"))]
mod windows;

#[cfg(feature = "client")]
pub(crate) mod options;

/// How much of a burst the writer buffers before handing it to the sink.
///
/// Once this much is buffered, it is written out before the next message is
/// added, so a long burst of small messages goes out in pieces of about this
/// size, one write each, rather than one write per message or all at once.
const WRITE_CHUNK: usize = 64 * 1024;

/// The most capacity a stdio buffer keeps once it is done with a message.
///
/// The reader's line and the writer's burst each grow to fit the largest
/// message they have carried, and would otherwise keep that memory for the
/// life of the process. Well above what ordinary traffic needs, so the common
/// path never pays for it: the trim is one reallocation, after a message large
/// enough that it is noise.
const RETAINED_CAPACITY: usize = 1024 * 1024;

/// Represents stdio server transport
#[cfg(feature = "server")]
pub(crate) struct StdIoServer {
    sender: StdIoSender,
    receiver: StdIoReceiver,
    /// One end of an in-memory pipe to read and write instead of the process's
    /// stdin and stdout. Taken by `start`.
    #[cfg(feature = "testing")]
    link: Option<DuplexStream>,
}

/// Represents stdio client transport
#[cfg(feature = "client")]
pub(crate) struct StdIoClient {
    sender: StdIoSender,
    receiver: StdIoReceiver,
    options: StdIoOptions,
    /// One end of an in-memory pipe to read and write instead of a child
    /// process's stdout and stdin. Taken by `start`.
    #[cfg(feature = "testing")]
    link: Option<DuplexStream>,
}

/// Represents stdio sender
pub(crate) struct StdIoSender {
    tx: Sender<Message>,
    rx: Option<Receiver<Message>>,
}

/// Represents stdio receiver
pub(crate) struct StdIoReceiver {
    tx: Sender<Result<Message, Error>>,
    rx: Receiver<Result<Message, Error>>,
}

impl Clone for StdIoSender {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            rx: None,
        }
    }
}

impl StdIoSender {
    /// Creates a new stdio transport sender
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::channel(100);
        Self { tx, rx: Some(rx) }
    }

    /// Starts a task that writes the queued messages to `writer`, one line of
    /// JSON each.
    ///
    /// For a sink with a real async form: a child's stdin, or the in-memory
    /// link. The server's own stdout has none -- see
    /// [`start_blocking`](Self::start_blocking). `writer` is the raw sink and
    /// is not to be wrapped in a buffer of its own: the task keeps one, and
    /// flushes it once per burst rather than once per message (see
    /// [`Outbox::write_burst`]).
    ///
    /// `drained` is held for as long as this writer may still write, so a
    /// caller awaiting the transport's drain signal knows the queue reached
    /// the wire rather than merely reaching the channel. The task is handed
    /// back so that caller can also end it when the shutdown budget runs out.
    #[cfg(feature = "client")]
    pub(crate) fn start<T: AsyncWrite + Unpin + Send + 'static>(
        &mut self,
        writer: T,
        token: CancellationToken,
        drained: DrainGuard,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let Some(mut receiver) = self.rx.take() else {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "The stdout writer already in use");
            return None;
        };

        Some(tokio::spawn(async move {
            let mut outbox = Outbox::new(writer);
            while let Some(msg) = next_message(&token, &mut receiver).await {
                outbox.write_burst(msg, &mut receiver).await;
            }

            // Cancellation stops new work, it does not discard queued work.
            // What sits in the channel here was written by a handler that
            // finished before the teardown got this far -- notably the
            // graceful-close result of a `subscriptions/listen` stream, which
            // the shutdown drain waited for on purpose. `try_recv`, since the
            // senders outlive this task and `recv` would never return `None`;
            // the burst takes the rest of the queue, under the same one flush.
            if let Ok(msg) = receiver.try_recv() {
                outbox.write_burst(msg, &mut receiver).await;
            }

            // Last thing the writer does: everything queued is now on the
            // wire, so whoever is waiting on the transport to finish may stop
            // waiting. Explicit rather than left to the end of the scope --
            // the drop is the signal.
            drop(drained);
        }))
    }

    /// Starts a dedicated OS thread that writes the queued messages to
    /// `writer` with blocking I/O, one line of JSON each.
    ///
    /// Used for the server's own stdout, which -- unlike a child's stdin --
    /// has no async form. [`tokio::io::stdout`] emulates one on the runtime's
    /// blocking pool, where every write and every flush is a round trip: two
    /// for a message on its own. A thread of our own writes it with one
    /// syscall instead, straight from the [`Outbox`] buffer, with no copy into
    /// tokio's.
    ///
    /// It also keeps shutdown from hanging on a peer that stopped reading. A
    /// write stuck on a full pipe cannot be interrupted either way, but on the
    /// blocking pool it is the runtime's, and dropping the runtime waits for
    /// it; this thread is nobody's to wait for, the reader's way (see
    /// [`StdIoReceiver::start_blocking`]).
    ///
    /// The thread waits on the channel and on `token` at once (see
    /// [`block_on`]): `blocking_recv` alone would sleep through a cancellation
    /// that arrives while nothing is queued, and shutdown would wait out its
    /// whole budget for a writer with nothing to write. On cancellation it
    /// drains what is queued, as the task does, then drops `drained`.
    ///
    /// Returns the token that stops the thread, for the caller to register
    /// with [`DrainSignal::stop_on_timeout`]: nothing can abort a thread, so
    /// one that runs out of shutdown budget is told to stop instead, which it
    /// checks before each write.
    #[cfg(feature = "server")]
    pub(crate) fn start_blocking<W: std::io::Write + Send + 'static>(
        &mut self,
        writer: W,
        token: CancellationToken,
        drained: DrainGuard,
    ) -> Result<Option<CancellationToken>, Error> {
        let Some(mut receiver) = self.rx.take() else {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "The stdout writer already in use");
            return Ok(None);
        };

        let stop = CancellationToken::new();
        std::thread::Builder::new()
            .name("neva-stdout".into())
            .spawn({
                let stop = stop.clone();
                move || {
                    let mut outbox = Outbox::new(writer);
                    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
                    while let Some(msg) = block_on(&waker, next_message(&token, &mut receiver)) {
                        outbox.write_burst_blocking(msg, &mut receiver, &stop);
                    }

                    // The task's drain, for the task's reasons.
                    if let Ok(msg) = receiver.try_recv() {
                        outbox.write_burst_blocking(msg, &mut receiver, &stop);
                    }

                    // The signal, as in the task: explicit, because the drop
                    // is what the shutdown is waiting for.
                    drop(drained);
                }
            })
            .map_err(|err| {
                Error::new(
                    ErrorCode::InternalError,
                    format!("failed to start the stdout writer: {err}"),
                )
            })?;
        Ok(Some(stop))
    }
}

/// The next message to write, or `None` once the writer is to take no more:
/// `token` cancelled, or every sender gone.
///
/// Cancellation first: a writer told to stop drains what is queued with
/// `try_recv`, rather than taking one more here.
async fn next_message(token: &CancellationToken, queue: &mut Receiver<Message>) -> Option<Message> {
    tokio::select! {
        biased;
        _ = token.cancelled() => None,
        msg = queue.recv() => msg,
    }
}

/// Runs `future` to completion on the current thread, parking the thread
/// whenever the future waits.
///
/// What lets the stdout writer thread wait on the channel and its
/// cancellation token at once. Both wake whatever waker they were polled
/// with and need no runtime to do it -- tokio's own `blocking_recv` is a
/// `block_on` over the same `recv`. `waker` must unpark this thread: an
/// [`Unpark`] of [`std::thread::current`], made once per thread.
#[cfg(feature = "server")]
fn block_on<F: Future>(waker: &Waker, future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut cx = Context::from_waker(waker);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }
        // A wake that lands before this park is not lost: it leaves the
        // thread's token set, and `park` returns at once. A spurious return
        // just polls again.
        std::thread::park();
    }
}

/// A waker that unparks one thread -- the one [`block_on`] is parked on.
#[cfg(feature = "server")]
struct Unpark(std::thread::Thread);

#[cfg(feature = "server")]
impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// The writer's end of the transport: the sink, and the one buffer every
/// message is serialized into on its way there.
///
/// One buffer for the life of the writer, rather than a fresh `Vec` per
/// message that a large message also regrows through every doubling. It is
/// also the only buffer: the bytes go from here straight to the sink, with no
/// `BufWriter` in between to copy them once more.
///
/// The burst itself is the same for both writers; only the I/O differs --
/// async for the task ([`write_burst`](Self::write_burst)), blocking for the
/// thread ([`write_burst_blocking`](Self::write_burst_blocking)).
struct Outbox<T> {
    sink: T,
    buf: Vec<u8>,
}

impl<T> Outbox<T> {
    /// An outbox with nothing buffered yet.
    fn new(sink: T) -> Self {
        Self {
            sink,
            buf: Vec::new(),
        }
    }

    /// Appends `msg` to the buffer as one line of JSON.
    ///
    /// A message that cannot be serialized is that message's problem: whatever
    /// part of it was already written is taken back out, so the lines on
    /// either side of it still reach the peer whole.
    fn encode(&mut self, msg: &impl serde::Serialize) {
        let start = self.buf.len();
        match serde_json::to_writer(&mut self.buf, msg) {
            Ok(()) => self.buf.push(b'\n'),
            Err(_err) => {
                self.buf.truncate(start);
                #[cfg(feature = "tracing")]
                tracing::error!(logger = "neva", "Serialization error: {:?}", _err);
            }
        }
    }

    /// Empties the buffer once what it held is written (or dropped), giving
    /// back what an unusually large message grew it by.
    fn recycle(&mut self) {
        self.buf.clear();
        self.buf.shrink_to(RETAINED_CAPACITY);
    }
}

#[cfg(feature = "client")]
impl<T: AsyncWrite + Unpin> Outbox<T> {
    /// Writes `first` and everything queued behind it, then flushes once.
    ///
    /// A message on its own is flushed as soon as it is written. A burst --
    /// progress or log notifications queued faster than the sink takes them --
    /// shares one flush instead of paying one each, and its small messages
    /// share a write. The order on the wire is the channel's.
    ///
    /// Neither failure is fatal to the writer: see [`encode`](Self::encode)
    /// and [`write_out`](Self::write_out).
    async fn write_burst(&mut self, first: Message, queue: &mut Receiver<Message>) {
        self.encode(&first);
        while let Ok(msg) = queue.try_recv() {
            if self.buf.len() >= WRITE_CHUNK {
                self.write_out().await;
            }
            self.encode(&msg);
        }
        self.write_out().await;
        let _ = self.sink.flush().await;
    }

    /// Hands everything buffered to the sink, leaving the buffer empty.
    ///
    /// A write error is logged and those bytes are dropped; a sink that is
    /// gone for good is reported by the next write too.
    async fn write_out(&mut self) {
        if let Err(_err) = self.sink.write_all(&self.buf).await {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "stdout write error: {:?}", _err);
        }
        self.recycle();
    }
}

#[cfg(feature = "server")]
impl<T: std::io::Write> Outbox<T> {
    /// [`write_burst`](Self::write_burst) for the writer thread: the same
    /// burst and the same one flush, with blocking I/O -- and a `stop` that
    /// is checked before each write.
    ///
    /// Once `stop` is cancelled the rest of the burst is dropped unwritten:
    /// the shutdown budget is spent, and output past it would land after the
    /// server said it had stopped.
    fn write_burst_blocking(
        &mut self,
        first: Message,
        queue: &mut Receiver<Message>,
        stop: &CancellationToken,
    ) {
        self.encode(&first);
        while let Ok(msg) = queue.try_recv() {
            if self.buf.len() >= WRITE_CHUNK && !self.write_out_blocking(stop) {
                return;
            }
            self.encode(&msg);
        }
        if self.write_out_blocking(stop) {
            let _ = self.sink.flush();
        }
    }

    /// Hands everything buffered to the sink unless `stop` is cancelled,
    /// leaving the buffer empty either way. Returns whether it wrote.
    ///
    /// A write error is logged and those bytes are dropped, as in
    /// [`write_out`](Self::write_out).
    fn write_out_blocking(&mut self, stop: &CancellationToken) -> bool {
        let stopped = stop.is_cancelled();
        if !stopped && let Err(_err) = self.sink.write_all(&self.buf) {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "stdout write error: {:?}", _err);
        }
        self.recycle();
        !stopped
    }
}

/// The direction an unreadable line has to travel -- a parse failure is
/// answered or completed depending on what the line *was*, and routing it
/// the wrong way loses it silently.
enum Line {
    /// A readable message -- hand it to the receive loop.
    Message(Message),
    /// An unreadable inbound **request**: JSON-RPC 2.0 section 5 says the peer
    /// gets an error response, so this goes straight back out the
    /// transport's sender rather than into pending-request handling.
    Reply(Message),
    /// Nothing actionable -- logged and dropped.
    Drop,
}

/// Parses one stdio line into a [`Message`].
///
/// A line that isn't a readable `Message` -- malformed JSON, or JSON that is
/// not JSON-RPC -- must **not** tear down the receive loop: the peer is alive
/// and every following line is still readable. Pushing a bare `Err` did exactly that, and since the loop
/// died before completing the pending request, the caller saw a timeout
/// instead of the parse failure.
///
/// A malformed **response** carrying an `id` belongs to a request this side
/// is waiting on: it is reported as an id-bound `ParseError` so the pending
/// request completes with the real cause. That is what makes the 2026-07-28
/// client's dual-mode fallback reachable over stdio -- it classifies such a
/// rejection as "legacy peer" and retries `initialize`, which a timeout
/// never could.
///
/// A malformed **request** (it carries `method`) is the mirror image: no
/// pending request to complete, so it is answered with a `ParseError`
/// response. Routing it into pending-request handling instead would
/// silently swallow it and leave the peer waiting out its own timeout.
///
/// A line whose JSON is broken outright can't be classified at all, so
/// JSON-RPC 2.0 section 5.1 prescribes the answer directly: a parse error with
/// [`RequestId::Null`](crate::types::RequestId::Null).
///
/// Dropped, because no move applies:
///
/// * a malformed **notification** -- a `method` but no `id`. JSON-RPC 2.0
///   section 4.1 forbids replying to notifications;
/// * a response-shaped line with no usable `id` -- it completes no pending
///   request, and answering a response is not a thing.
fn parse_line(line: &str) -> Line {
    let err = match serde_json::from_str::<Message>(line) {
        Ok(msg) => return Line::Message(msg),
        Err(err) => err,
    };

    let reply = |id| {
        Message::Response(crate::types::Response::error(
            id,
            Error::new(ErrorCode::ParseError, err.to_string()),
        ))
    };

    // Broken JSON: nothing to classify, and section 5.1 answers exactly this case.
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return Line::Reply(reply(crate::types::RequestId::Null));
    };

    // A `method` makes it a request/notification, never a response.
    let is_request = value.get("method").is_some();
    let id = value
        .get("id")
        .cloned()
        .and_then(|id| serde_json::from_value::<crate::types::RequestId>(id).ok())
        // An absent id and an explicit `null` are equally unaddressable.
        .filter(|id| !matches!(id, crate::types::RequestId::Null));

    match (is_request, id) {
        // An invalid request is answered with its own id...
        (true, Some(id)) => Line::Reply(reply(id)),
        // ...but a notification is never answered at all.
        (true, None) => {
            #[cfg(feature = "tracing")]
            tracing::error!(
                logger = "neva",
                "Dropped a malformed stdio notification: {}",
                err
            );
            Line::Drop
        }
        // A malformed response completes the request it belongs to.
        (false, Some(id)) => Line::Message(reply(id)),
        (false, None) => {
            #[cfg(feature = "tracing")]
            tracing::error!(
                logger = "neva",
                "Dropped an unaddressed stdio response: {}",
                err
            );
            Line::Drop
        }
    }
}

/// What a receive loop does with the line it just read.
///
/// The two loops below read over different I/O -- a client awaits a child's
/// piped stdout, a server blocks on its own stdin -- but what a line *means*
/// is the same either way, so it is decided once here and each loop only
/// carries it out with the sends its own flavour has.
enum Step {
    /// Nothing to forward; read the next line.
    Skip,
    /// Hand to the dispatch layer.
    Forward(Message),
    /// Answer the peer directly through the transport's sender.
    Answer(Message),
    /// Report the read failure, then stop.
    Fail(Error),
    /// Input is finished.
    Stop,
}

/// Classifies one read: how it went, and what the line it produced was.
#[inline]
fn next_step(read: std::io::Result<usize>, line: &str) -> Step {
    match read {
        Ok(0) => Step::Stop, // EOF
        Ok(_) => match parse_line(line) {
            Line::Drop => Step::Skip,
            Line::Message(msg) => Step::Forward(msg),
            Line::Reply(resp) => Step::Answer(resp),
        },
        Err(err) => Step::Fail(err.into()),
    }
}

impl StdIoReceiver {
    /// Creates a new stdio transport receiver
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::channel(100);
        Self { tx, rx }
    }

    /// Starts a task that reads `reader` asynchronously.
    ///
    /// Serves a client's view of a child process's piped stdout: a real async
    /// pipe, so a pending read is a task the runtime can simply drop, and
    /// cancellation lands on the next line boundary or sooner.
    ///
    /// `replies` is the transport's own sender: an unreadable inbound
    /// request is answered with a JSON-RPC parse error from here, since
    /// it never reaches the dispatch layer that would normally reply.
    #[cfg(feature = "client")]
    pub(crate) fn start<T: AsyncRead + Unpin + Send + 'static>(
        &self,
        mut reader: BufReader<T>,
        replies: Sender<Message>,
        token: CancellationToken,
    ) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut line = String::new();
            loop {
                line.clear();
                line.shrink_to(RETAINED_CAPACITY);
                let read = tokio::select! {
                    biased;
                    _ = token.cancelled() => break,
                    read = reader.read_line(&mut line) => read,
                };
                let sent = match next_step(read, &line) {
                    Step::Skip => continue,
                    Step::Stop => break,
                    Step::Forward(msg) => tx.send(Ok(msg)).await.is_ok(),
                    Step::Answer(resp) => replies.send(resp).await.is_ok(),
                    Step::Fail(err) => {
                        let _ = tx.send(Err(err)).await;
                        break;
                    }
                };
                if !sent {
                    break;
                }
            }
        });
    }

    /// Starts a dedicated OS thread that reads `reader` with blocking I/O.
    ///
    /// Used for the server's own `stdin`, which -- unlike a child process's
    /// piped stdout -- has no async form. [`tokio::io::stdin`] emulates one by
    /// parking each read on the runtime's *blocking pool*, and a read parked
    /// there when the shutdown signal arrives is never interrupted: the peer
    /// simply is not writing. Dropping the runtime waits for that pool, so a
    /// server that shut down cleanly would still hang the process until the
    /// peer happened to send another line. Ctrl+C looked like it did nothing.
    ///
    /// A thread of our own is not the runtime's to wait for, so returning from
    /// `main` ends the process while this thread is still parked in `read` --
    /// which is also what [`tokio::io::stdin`]'s own documentation recommends
    /// for interactive input, for this reason.
    ///
    /// # What this does not solve
    ///
    /// A read already in progress still cannot be interrupted; tokio says as
    /// much of its own stdin ("it is impossible to cancel that read"), and no
    /// portable, safe API changes that. Cancellation is therefore observed
    /// *between* lines. For a server that owns its process this is invisible:
    /// nothing outlives the parked read. For a host that keeps running after
    /// `App::run` returns it is not -- the parked thread stays attached to
    /// stdin and swallows the next line before noticing it has nowhere to put
    /// it, so a restarted server, or the host reading stdin itself, loses that
    /// line.
    ///
    /// End of input has the mirror problem: the receiver still holds a sender
    /// of its own, so the channel never closes and the dispatch loop waits on
    /// input that can no longer come.
    ///
    /// Both belong to the same missing piece -- an stdio transport that can be
    /// *shut down* rather than merely abandoned: drain the handlers still in
    /// flight so their answers are not thrown away, close the stream, and hand
    /// stdin back. Neither is introduced here, and neither is conflated with
    /// the Ctrl+C fix.
    #[cfg(feature = "server")]
    pub(crate) fn start_blocking<T: std::io::BufRead + Send + 'static>(
        &self,
        mut reader: T,
        replies: Sender<Message>,
        token: CancellationToken,
    ) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let mut line = String::new();
            loop {
                line.clear();
                line.shrink_to(RETAINED_CAPACITY);
                if token.is_cancelled() {
                    break;
                }
                let read = reader.read_line(&mut line);
                let sent = match next_step(read, &line) {
                    Step::Skip => continue,
                    Step::Stop => break,
                    Step::Forward(msg) => tx.blocking_send(Ok(msg)).is_ok(),
                    Step::Answer(resp) => replies.blocking_send(resp).is_ok(),
                    Step::Fail(err) => {
                        let _ = tx.blocking_send(Err(err));
                        break;
                    }
                };
                if !sent {
                    break;
                }
            }
        });
    }
}

#[cfg(feature = "client")]
impl StdIoClient {
    /// Creates a new stdio transport for this client
    pub(crate) fn new(options: StdIoOptions) -> Self {
        Self {
            receiver: StdIoReceiver::new(),
            sender: StdIoSender::new(),
            options,
            #[cfg(feature = "testing")]
            link: None,
        }
    }

    /// Creates a stdio transport that talks over `link` and spawns no process.
    #[cfg(feature = "testing")]
    pub(crate) fn in_memory(link: DuplexStream) -> Self {
        Self {
            link: Some(link),
            ..Self::new(StdIoOptions::new("", []))
        }
    }

    /// Handshakes stdio between client and server apps
    ///
    /// Returns an [`Error`] when the configured command cannot be spawned:
    /// a typo, a binary that was never built, a server removed from `PATH`.
    /// See <https://github.com/RomanEmreis/neva/issues/125>.
    fn handshake(
        &self,
        token: CancellationToken,
    ) -> Result<(BufReader<ChildStdout>, ChildStdin), Error> {
        let options = &self.options;
        // `&dyn Display` rather than a concrete error type: each backend below
        // chooses its own, and this closure should not have to be touched when
        // one of them changes it.
        let spawn_failed = |err: &dyn std::fmt::Display| {
            Error::new(
                ErrorCode::InternalError,
                format!("failed to start MCP server `{}`: {err}", options.command),
            )
        };
        #[cfg(target_os = "linux")]
        let (job, mut child) =
            linux::Job::new(options.command, &options.args).map_err(|e| spawn_failed(&e))?;
        #[cfg(target_os = "windows")]
        let (job, mut child) =
            windows::Job::new(options.command, &options.args).map_err(|e| spawn_failed(&e))?;
        #[cfg(all(not(target_os = "windows"), not(target_os = "linux")))]
        let mut child = tokio::process::Command::new(options.command)
            .args(&options.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| spawn_failed(&e))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| spawn_failed(&"inaccessible stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| spawn_failed(&"inaccessible stdout"))?;

        #[cfg(feature = "tracing")]
        let child_id = child.id();

        tokio::task::spawn(async move {
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            let _job = job;
            tokio::select! {
                biased;
                _ = child.wait() => {}
                _ = token.cancelled() => {
                    if let Err(_e) = child.kill().await {
                        #[cfg(feature = "tracing")]
                        tracing::warn!(
                            logger = "neva",
                            pid = child_id,
                            "Failed to kill child process: {:?}", _e);
                    } else {
                        let _exit = child.wait().await;
                        #[cfg(feature = "tracing")]
                        tracing::trace!(
                            logger = "neva",
                            pid = child_id,
                            "Child exited with status: {:?}", _exit);
                    }
                },
            }
        });

        Ok((BufReader::new(stdout), stdin))
    }
}

#[cfg(feature = "server")]
impl StdIoServer {
    /// Creates a new stdio transport for server
    pub(crate) fn new() -> Self {
        Self {
            receiver: StdIoReceiver::new(),
            sender: StdIoSender::new(),
            #[cfg(feature = "testing")]
            link: None,
        }
    }

    /// Creates a stdio transport that talks over `link` and leaves the
    /// process's own stdin and stdout alone.
    #[cfg(feature = "testing")]
    pub(crate) fn in_memory(link: DuplexStream) -> Self {
        Self {
            link: Some(link),
            ..Self::new()
        }
    }

    /// Initializes and Returns handles to `stdin` and `stdout`.
    ///
    /// Both are the std ones, each served by a thread of our own: see
    /// [`StdIoReceiver::start_blocking`] and [`StdIoSender::start_blocking`]
    /// for why tokio's are not used here. `stdout` is not buffered here: the
    /// writer keeps a buffer of its own, and every burst it writes ends on a
    /// newline, which std's line-buffered stdout passes straight through.
    pub(crate) fn init() -> (std::io::BufReader<std::io::Stdin>, std::io::Stdout) {
        (std::io::BufReader::new(std::io::stdin()), std::io::stdout())
    }
}

impl TransportSender for StdIoSender {
    async fn send(&self, msg: Message) -> Result<(), Error> {
        self.tx
            .send(msg)
            .map_err(|err| Error::new(ErrorCode::InternalError, err))
            .await
    }
}

impl TransportReceiver for StdIoReceiver {
    async fn recv(&mut self) -> Result<Message, Error> {
        self.rx.recv().await.unwrap_or_else(|| {
            Err(Error::new(
                ErrorCode::InvalidRequest,
                "Unexpected end of stream",
            ))
        })
    }
}

/// Starts the reader and writer tasks of either side over one end of an
/// in-memory pipe.
///
/// A client and a server run the same two here: there is no child process to
/// spawn, and no stdin to park a thread on.
#[cfg(feature = "testing")]
fn start_in_memory(
    link: DuplexStream,
    receiver: &StdIoReceiver,
    sender: &mut StdIoSender,
) -> TransportHandle {
    let token = CancellationToken::new();
    let (guard, mut drained) = DrainSignal::new();
    let (reader, writer) = tokio::io::split(link);
    receiver.start(BufReader::new(reader), sender.tx.clone(), token.clone());
    if let Some(task) = sender.start(writer, token.clone(), guard) {
        drained.abort_on_timeout(task.abort_handle());
    }
    TransportHandle::new(token, drained)
}

#[cfg(feature = "client")]
impl Transport for StdIoClient {
    type Sender = StdIoSender;
    type Receiver = StdIoReceiver;

    fn start(&mut self) -> Result<TransportHandle, Error> {
        #[cfg(feature = "testing")]
        if let Some(link) = self.link.take() {
            return Ok(start_in_memory(link, &self.receiver, &mut self.sender));
        }

        let token = CancellationToken::new();
        let (reader, writer) = self.handshake(token.clone())?;
        let (guard, mut drained) = DrainSignal::new();

        self.receiver
            .start(reader, self.sender.tx.clone(), token.clone());

        if let Some(task) = self.sender.start(writer, token.clone(), guard) {
            drained.abort_on_timeout(task.abort_handle());
        }

        #[cfg(feature = "tracing")]
        tracing::info!(logger = "neva", "Connected: stdio");
        Ok(TransportHandle::new(token, drained))
    }

    #[inline]
    fn split(self) -> (Self::Sender, Self::Receiver) {
        (self.sender, self.receiver)
    }
}

#[cfg(feature = "server")]
impl Transport for StdIoServer {
    type Sender = StdIoSender;
    type Receiver = StdIoReceiver;

    fn start(&mut self) -> Result<TransportHandle, Error> {
        #[cfg(feature = "testing")]
        if let Some(link) = self.link.take() {
            return Ok(start_in_memory(link, &self.receiver, &mut self.sender));
        }

        let token = CancellationToken::new();
        let (reader, writer) = StdIoServer::init();
        // Only the writer takes a guard. Both are threads of neva's own, but
        // the reader is parked in a read nothing can interrupt, and shutdown
        // was never allowed to wait for it. The writer checks in between
        // writes, so it can be both waited for and stopped.
        let (guard, mut drained) = DrainSignal::new();

        // The writer first: it is the one that can fail to start, and failing
        // must not leave a reader thread behind with nobody to cancel it.
        if let Some(stop) = self.sender.start_blocking(writer, token.clone(), guard)? {
            drained.stop_on_timeout(stop);
        }

        self.receiver
            .start_blocking(reader, self.sender.tx.clone(), token.clone());

        #[cfg(feature = "tracing")]
        tracing::info!(logger = "neva", "Listening: stdio");
        Ok(TransportHandle::new(token, drained))
    }

    #[inline]
    fn split(self) -> (Self::Sender, Self::Receiver) {
        (self.sender, self.receiver)
    }
}

#[cfg(test)]
mod parse_line_tests {
    use super::*;
    use crate::types::RequestId;

    /// A JSON-RPC error whose code neva has no variant for (the TS SDK's
    /// `-32000`, the reply a legacy server sends to the 2026-07-28 client's
    /// pre-initialize `server/discover`) is a readable response: it completes
    /// the pending request with the code and message the peer sent (#171).
    #[test]
    fn an_unknown_error_code_completes_the_request_as_sent() {
        let line = r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32000,"message":"Server not initialized"}}"#;

        let Line::Message(Message::Response(crate::types::Response::Err(resp))) = parse_line(line)
        else {
            panic!("the response must complete the pending request");
        };
        assert_eq!(resp.id, RequestId::Number(7));
        assert_eq!(resp.error.code, ErrorCode::Other(-32000));
        assert_eq!(resp.error.message, "Server not initialized");
    }

    /// A response whose JSON is fine but is not JSON-RPC still completes the
    /// pending request, as an id-bound `ParseError`.
    #[test]
    fn a_malformed_response_becomes_an_id_bound_parse_error() {
        let line = r#"{"jsonrpc":"2.0","id":7,"error":{"code":"not a number"}}"#;

        let Line::Message(Message::Response(crate::types::Response::Err(resp))) = parse_line(line)
        else {
            panic!("a malformed response must complete the pending request");
        };
        assert_eq!(resp.id, RequestId::Number(7));
        assert_eq!(resp.error.code, ErrorCode::ParseError);
    }

    /// The regression the fix is really about: the bad line must not end
    /// the stream -- the pending request completes *and* the following
    /// lines keep arriving.
    // Drives the async reader, which serves a child process's piped stdout and
    // so is compiled only for a client. `blocking_reader_tests` covers the same
    // routing on the server's own reader; both go through `next_step`.
    #[cfg(feature = "client")]
    #[tokio::test]
    async fn a_bad_line_neither_ends_the_stream_nor_is_swallowed() {
        const INPUT: &[u8] = concat!(
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":"not a number"}}"#,
            "\n",
            "total garbage, no id\n",
            r#"{"jsonrpc":"2.0","method":"ping"}"#,
            "\n",
        )
        .as_bytes();

        let (replies, _replies_rx) = mpsc::channel(8);
        let mut receiver = StdIoReceiver::new();
        receiver.start(BufReader::new(INPUT), replies, CancellationToken::new());

        let Ok(Message::Response(crate::types::Response::Err(resp))) = receiver.recv().await else {
            panic!("the malformed response must complete the pending request");
        };
        assert_eq!(resp.id, RequestId::Number(1));
        assert_eq!(resp.error.code, ErrorCode::ParseError);

        // The garbage line is answered out-of-band (section 5.1) rather than
        // routed here, so the next thing the loop sees is the good line.
        assert!(
            matches!(receiver.recv().await, Ok(Message::Notification(_))),
            "the stream must survive both bad lines"
        );
    }

    /// End-to-end through the reader task: a malformed inbound request is
    /// written back out the transport's sender, and never surfaces to the
    /// receive loop that would have swallowed it.
    #[cfg(feature = "client")]
    #[tokio::test]
    async fn a_malformed_request_is_written_back_to_the_peer() {
        const INPUT: &[u8] = concat!(
            r#"{"jsonrpc":"2.0","id":42,"method":123}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"ping"}"#,
            "\n",
        )
        .as_bytes();

        let (replies, mut replies_rx) = mpsc::channel(8);
        let mut receiver = StdIoReceiver::new();
        receiver.start(BufReader::new(INPUT), replies, CancellationToken::new());

        let Some(Message::Response(crate::types::Response::Err(resp))) = replies_rx.recv().await
        else {
            panic!("the peer must receive a JSON-RPC error for its bad request");
        };
        assert_eq!(resp.id, RequestId::Number(42));
        assert_eq!(resp.error.code, ErrorCode::ParseError);

        // The bad request never reaches the receive loop -- the next thing
        // it sees is the following, well-formed line.
        assert!(
            matches!(receiver.recv().await, Ok(Message::Notification(_))),
            "a malformed request must not be routed into pending handling"
        );
    }

    /// JSON-RPC 2.0 section 5.1: invalid JSON is answered with a parse error
    /// carrying `"id": null`, since there is no id to salvage.
    #[test]
    fn broken_json_is_answered_with_a_null_id() {
        for line in [
            r#"{"jsonrpc":"2.0","id":"abc","result":}"#,
            "not json at all",
            "{",
        ] {
            let Line::Reply(Message::Response(crate::types::Response::Err(resp))) =
                parse_line(line)
            else {
                panic!("broken JSON must be answered per section 5.1: {line}");
            };
            assert_eq!(resp.id, RequestId::Null);
            assert_eq!(resp.error.code, ErrorCode::ParseError);
            // The reply must go out with a literal JSON `null` id.
            let wire = serde_json::to_value(&resp).unwrap();
            assert!(wire["id"].is_null(), "id must serialize as null: {wire}");
        }
    }

    #[test]
    fn an_id_is_salvaged_when_the_line_is_still_valid_json() {
        // Valid JSON that simply isn't a `Message`.
        let line = r#"{"jsonrpc":"2.0","id":"abc","totally":"unknown"}"#;
        let Line::Message(Message::Response(crate::types::Response::Err(resp))) = parse_line(line)
        else {
            panic!("an addressed parse failure must produce an error response");
        };
        assert_eq!(resp.id, RequestId::String("abc".into()));
    }

    #[test]
    fn unanswerable_lines_are_dropped_not_fatal() {
        // A `method` but no `id` -- a notification, which JSON-RPC section 4.1
        // forbids replying to. An explicit `null` id is just as unaddressable.
        for line in [
            r#"{"jsonrpc":"2.0","method":123}"#,
            r#"{"jsonrpc":"2.0","id":null,"method":123}"#,
        ] {
            assert!(
                matches!(parse_line(line), Line::Drop),
                "a malformed notification must never be answered: {line}"
            );
        }

        // Response-shaped, but completes no pending request.
        assert!(matches!(
            parse_line(r#"{"jsonrpc":"2.0","totally":"unknown"}"#),
            Line::Drop
        ));
    }

    /// A malformed inbound *request* carries `method`, so it completes no
    /// pending request. Routing it into pending-request handling would
    /// swallow it; the peer gets a JSON-RPC parse error instead.
    #[test]
    fn malformed_requests_are_answered_not_swallowed() {
        let malformed_requests = [
            (
                r#"{"jsonrpc":"2.0","id":3,"method":123}"#,
                RequestId::Number(3),
            ),
            (
                r#"{"jsonrpc":"2.0","id":3,"method":{"nested":"object"}}"#,
                RequestId::Number(3),
            ),
            (
                r#"{"jsonrpc":"2.0","id":"abc","method":null,"params":{}}"#,
                RequestId::String("abc".into()),
            ),
        ];

        for (line, expected_id) in malformed_requests {
            let Line::Reply(Message::Response(crate::types::Response::Err(resp))) =
                parse_line(line)
            else {
                panic!("a malformed request must be answered, not routed: {line}");
            };
            assert_eq!(resp.id, expected_id);
            assert_eq!(resp.error.code, ErrorCode::ParseError);
        }
    }

    #[test]
    fn well_formed_lines_are_unaffected() {
        let line = r#"{"jsonrpc":"2.0","method":"ping"}"#;
        assert!(matches!(
            parse_line(line),
            Line::Message(Message::Notification(_))
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::StdIoSender;
    use crate::transport::{DrainGuard, DrainSignal};
    use crate::types::Message;
    use tokio_util::sync::CancellationToken;

    /// A sink the test can read back while a writer owns it: async for the
    /// task, blocking for the thread.
    #[derive(Clone, Default)]
    struct SharedSink(std::sync::Arc<std::sync::Mutex<Recorded>>);

    /// Everything a [`SharedSink`] was handed, and how.
    #[derive(Default)]
    struct Recorded {
        bytes: Vec<u8>,
        writes: usize,
        flushes: usize,
    }

    impl SharedSink {
        /// Reads what has been recorded so far.
        fn read<R>(&self, f: impl FnOnce(&Recorded) -> R) -> R {
            match self.0.lock() {
                Ok(recorded) => f(&recorded),
                Err(poisoned) => f(&poisoned.into_inner()),
            }
        }

        /// How many complete lines have been written so far.
        fn lines(&self) -> usize {
            self.read(|r| r.bytes.iter().filter(|b| **b == b'\n').count())
        }

        /// The `seq` of every line written so far, in the order written.
        fn seqs(&self) -> Vec<u64> {
            self.read(|r| seqs(&r.bytes))
        }

        /// Records one write of `buf`, taken whole.
        fn record_write(&self, buf: &[u8]) {
            if let Ok(mut sink) = self.0.lock() {
                sink.bytes.extend_from_slice(buf);
                sink.writes += 1;
            }
        }

        /// Records one flush.
        fn record_flush(&self) {
            if let Ok(mut sink) = self.0.lock() {
                sink.flushes += 1;
            }
        }
    }

    impl tokio::io::AsyncWrite for SharedSink {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            self.record_write(buf);
            std::task::Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            self.record_flush();
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    impl std::io::Write for SharedSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.record_write(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.record_flush();
            Ok(())
        }
    }

    /// The `seq` of every line in `bytes`, in order. Panics on a line that is
    /// not whole JSON.
    fn seqs(bytes: &[u8]) -> Vec<u64> {
        bytes
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| {
                let json: serde_json::Value =
                    serde_json::from_slice(line).expect("each line is whole JSON");
                json["params"]["seq"].as_u64().expect("a seq")
            })
            .collect()
    }

    /// Shutdown cancels the writer, but whatever a handler already queued still
    /// has to reach stdout -- the graceful-close result of a
    /// `subscriptions/listen` stream is written in exactly that window, after
    /// the drain phase woke the handler and before the transport goes down.
    /// Breaking out of the loop on cancellation without draining is what used
    /// to lose it.
    #[cfg(feature = "client")]
    #[tokio::test]
    async fn a_cancelled_writer_still_flushes_what_was_queued() {
        use super::StdIoSender;
        use crate::transport::DrainSignal;
        use crate::types::{Message, notification::Notification};
        use tokio_util::sync::CancellationToken;

        const QUEUED: usize = 3;

        let mut sender = StdIoSender::new();
        let tx = sender.tx.clone();

        // Queued before the writer starts, so all of it is sitting in the
        // channel by the time cancellation is observed.
        for i in 0..QUEUED {
            tx.send(Message::Notification(Notification::new(
                "notifications/message",
                Some(serde_json::json!({ "seq": i })),
            )))
            .await
            .expect("the channel has room");
        }

        // Cancelled up front: the writer's very first poll takes the `biased`
        // cancellation arm, which is the case that used to drop the queue.
        let token = CancellationToken::new();
        token.cancel();

        let sink = SharedSink::default();
        let (guard, _drained) = DrainSignal::new();
        sender.start(sink.clone(), token, guard);

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while sink.lines() < QUEUED {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "a cancelled writer must drain the queue instead of dropping it: \
                 {} of {QUEUED} messages reached the sink",
                sink.lines()
            )
        });
    }

    /// A sink that takes its time, the way stdout with a slow reader behind
    /// it does. Without it the whole question is invisible: an instant write
    /// finishes inside the moment between the shutdown signal and the runtime
    /// going away, and the drain nobody waited for looks like a drain that
    /// worked.
    #[cfg(feature = "client")]
    struct SlowSink {
        sink: SharedSink,
        delay: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
    }

    #[cfg(feature = "client")]
    impl SlowSink {
        /// Long enough that no write completes by accident, short enough that
        /// the whole queue still clears well inside a test's patience.
        const WRITE_DELAY: std::time::Duration = std::time::Duration::from_millis(50);

        fn new(sink: SharedSink) -> Self {
            Self { sink, delay: None }
        }
    }

    #[cfg(feature = "client")]
    impl tokio::io::AsyncWrite for SlowSink {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            let this = self.get_mut();
            let delay = this
                .delay
                .get_or_insert_with(|| Box::pin(tokio::time::sleep(Self::WRITE_DELAY)));
            if std::future::Future::poll(delay.as_mut(), cx).is_pending() {
                return std::task::Poll::Pending;
            }
            this.delay = None;
            std::pin::Pin::new(&mut this.sink).poll_write(cx, buf)
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    /// One shutdown on a runtime of its own -- what `App::run_blocking` does --
    /// returning how much of the queue reached the sink after the runtime was
    /// dropped. `join` is the fix under test: whether the shutdown waits for
    /// the writer's drain signal before letting the runtime go.
    #[cfg(feature = "client")]
    fn shutdown_on_an_owned_runtime(queued: usize, join: bool) -> usize {
        use super::StdIoSender;
        use crate::transport::DrainSignal;
        use crate::types::{Message, notification::Notification};
        use tokio_util::sync::CancellationToken;

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("a runtime for the server");
        let sink = SharedSink::default();

        runtime.block_on(async {
            let mut sender = StdIoSender::new();
            for i in 0..queued {
                sender
                    .tx
                    .send(Message::Notification(Notification::new(
                        "notifications/message",
                        Some(serde_json::json!({ "seq": i })),
                    )))
                    .await
                    .expect("the channel has room");
            }

            let token = CancellationToken::new();
            let (guard, drained) = DrainSignal::new();
            sender.start(SlowSink::new(sink.clone()), token.clone(), guard);

            // Shutdown reaching the transport: the writer starts draining what
            // is queued, and the server's loop is over.
            token.cancel();

            if join {
                assert!(
                    drained
                        .wait_or_abort(std::time::Duration::from_secs(5))
                        .await,
                    "the writer must raise its drain signal well inside the budget"
                );
            }
        });

        // `run_blocking` drops its runtime here, and a dropped runtime aborts
        // whatever has not finished.
        drop(runtime);
        sink.lines()
    }

    /// The drain the writer performs on cancellation is worth nothing on its
    /// own: `App::run_blocking` drops its runtime as soon as `run` returns,
    /// and that abandons a writer still mid-drain. Waiting for the writer's
    /// signal first is what gets the queue -- the graceful-close result of a
    /// `subscriptions/listen` stream among it -- onto the wire.
    #[cfg(feature = "client")]
    #[test]
    fn waiting_for_the_writer_is_what_survives_the_runtime_being_dropped() {
        const QUEUED: usize = 3;

        let joined = shutdown_on_an_owned_runtime(QUEUED, true);
        let abandoned = shutdown_on_an_owned_runtime(QUEUED, false);

        assert_eq!(
            joined, QUEUED,
            "a shutdown that waits for the writer must have written all {QUEUED} messages"
        );
        assert_eq!(
            abandoned, 0,
            "and one that does not is the hazard: the runtime goes away mid-drain"
        );
    }

    /// A log notification carrying its place in the queue, and `pad` bytes of
    /// payload to give it some size.
    fn numbered(seq: usize, pad: usize) -> Message {
        use crate::types::notification::Notification;

        Message::Notification(Notification::new(
            "notifications/message",
            Some(serde_json::json!({ "seq": seq, "pad": "x".repeat(pad) })),
        ))
    }

    /// Queues `count` messages on a sender no writer has taken yet, so the
    /// writer's first wake-up finds all of them waiting.
    async fn queued(count: usize, pad: usize) -> StdIoSender {
        let sender = StdIoSender::new();
        for seq in 0..count {
            sender
                .tx
                .send(numbered(seq, pad))
                .await
                .expect("the channel has room");
        }
        sender
    }

    /// The two writers: a task for a sink with an async form, a thread for the
    /// server's own stdout. Each where its feature compiles it.
    #[derive(Clone, Copy, Debug)]
    enum Kind {
        #[cfg(feature = "client")]
        Task,
        #[cfg(feature = "server")]
        Thread,
    }

    /// Every writer this build has, for the tests that hold for both.
    const KINDS: &[Kind] = &[
        #[cfg(feature = "client")]
        Kind::Task,
        #[cfg(feature = "server")]
        Kind::Thread,
    ];

    /// Starts a writer of `kind` over `sender`'s queue and registers it with
    /// `drained`, the way its transport does.
    fn start_writer(
        kind: Kind,
        sender: &mut StdIoSender,
        sink: SharedSink,
        token: CancellationToken,
        guard: DrainGuard,
        drained: &mut DrainSignal,
    ) {
        match kind {
            #[cfg(feature = "client")]
            Kind::Task => {
                let task = sender.start(sink, token, guard).expect("a fresh sender");
                drained.abort_on_timeout(task.abort_handle());
            }
            #[cfg(feature = "server")]
            Kind::Thread => {
                let stop = sender
                    .start_blocking(sink, token, guard)
                    .expect("the thread starts")
                    .expect("a fresh sender");
                drained.stop_on_timeout(stop);
            }
        }
    }

    /// Runs a writer of `kind` over `sender`'s queue until it is empty and the
    /// writer is done -- on its own because every sender is gone, or draining
    /// on its way out if `cancelled`.
    async fn write_all_queued(kind: Kind, mut sender: StdIoSender, cancelled: bool) -> SharedSink {
        let token = CancellationToken::new();
        if cancelled {
            token.cancel();
        }
        let sink = SharedSink::default();
        let (guard, mut drained) = DrainSignal::new();
        start_writer(kind, &mut sender, sink.clone(), token, guard, &mut drained);

        // The last sender: with it gone, a running writer ends once the queue
        // is empty, and raises the drain signal on the way.
        drop(sender);
        assert!(
            drained
                .wait_or_abort(std::time::Duration::from_secs(5))
                .await,
            "the {kind:?} writer must finish the queue well inside the budget"
        );
        sink
    }

    /// What is already queued when the writer wakes goes out under one flush,
    /// in the order it was queued -- whether the writer is running or draining
    /// on its way out (#138). A flush per message cost the server's stdout a
    /// blocking-pool round trip per message.
    #[tokio::test]
    async fn a_queued_burst_reaches_the_sink_with_one_flush() {
        const QUEUED: usize = 10;

        for &kind in KINDS {
            for cancelled in [false, true] {
                let sink = write_all_queued(kind, queued(QUEUED, 0).await, cancelled).await;

                assert_eq!(
                    sink.seqs(),
                    (0..QUEUED as u64).collect::<Vec<_>>(),
                    "every queued message, in order ({kind:?}, cancelled: {cancelled})"
                );
                assert_eq!(
                    sink.read(|r| r.flushes),
                    1,
                    "one burst, one flush ({kind:?}, cancelled: {cancelled})"
                );
                assert_eq!(
                    sink.read(|r| r.writes),
                    1,
                    "small messages share a write ({kind:?}, cancelled: {cancelled})"
                );
            }
        }
    }

    /// Batching never holds a message back waiting for company: a message
    /// that arrives on its own is written and flushed before the writer goes
    /// back to waiting.
    #[tokio::test]
    async fn a_lone_message_is_flushed_without_waiting_for_more() {
        for &kind in KINDS {
            let mut sender = StdIoSender::new();
            let tx = sender.tx.clone();
            let sink = SharedSink::default();
            let (guard, mut drained) = DrainSignal::new();
            let token = CancellationToken::new();
            start_writer(
                kind,
                &mut sender,
                sink.clone(),
                token.clone(),
                guard,
                &mut drained,
            );

            for seq in 0..3 {
                tx.send(numbered(seq, 0))
                    .await
                    .expect("the writer is running");

                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    while sink.read(|r| r.flushes) <= seq {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap_or_else(|_| panic!("{kind:?}: message {seq} must be flushed on its own"));
                assert_eq!(sink.lines(), seq + 1, "{kind:?}");
            }

            token.cancel();
            assert!(
                drained
                    .wait_or_abort(std::time::Duration::from_secs(5))
                    .await
            );
        }
    }

    /// A burst too big for one piece is still written in pieces, so the
    /// writer never holds a whole queue of large messages in memory at once --
    /// but it still flushes once.
    #[tokio::test]
    async fn a_large_burst_is_written_in_pieces_under_one_flush() {
        const QUEUED: usize = 64;
        const PAD: usize = 4 * 1024;

        for &kind in KINDS {
            let sink = write_all_queued(kind, queued(QUEUED, PAD).await, false).await;

            let (bytes, writes, flushes) = sink.read(|r| (r.bytes.len(), r.writes, r.flushes));
            assert_eq!(sink.lines(), QUEUED, "{kind:?}");
            assert_eq!(flushes, 1, "{kind:?}: one burst, one flush");
            assert!(
                writes > 1,
                "{kind:?}: {bytes} bytes must not go out as one piece of {bytes}"
            );
            // Every write but the last carries at least a full chunk.
            assert!(
                writes <= bytes / super::WRITE_CHUNK + 1,
                "{kind:?}: {bytes} bytes in {writes} writes: the messages must share them"
            );
        }
    }

    /// A message that fails halfway through serializing does not leave half a
    /// line in the burst for the next message to be glued onto.
    #[test]
    fn a_message_that_cannot_be_serialized_leaves_its_neighbours_whole() {
        /// Starts an object, then fails inside it.
        struct FailsHalfway;

        impl serde::Serialize for FailsHalfway {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                use serde::ser::{Error, SerializeStruct};

                let mut object = s.serialize_struct("FailsHalfway", 2)?;
                object.serialize_field("written", "before the failure")?;
                Err(S::Error::custom("fails halfway"))
            }
        }

        let mut outbox = super::Outbox::new(());
        outbox.encode(&numbered(0, 0));
        outbox.encode(&FailsHalfway);
        outbox.encode(&numbered(1, 0));

        assert_eq!(
            seqs(&outbox.buf),
            [0, 1],
            "two whole lines and nothing between"
        );
    }

    /// The writer's buffer is kept from one burst to the next, except for
    /// what an unusually large message grew it by.
    #[test]
    fn the_writer_keeps_its_buffer_but_not_an_oversized_one() {
        let mut outbox = super::Outbox::new(());

        outbox.encode(&numbered(0, 0));
        outbox.recycle();
        let kept = outbox.buf.capacity();
        assert!(kept > 0, "an ordinary message's buffer is reused");

        outbox.encode(&numbered(1, 0));
        outbox.recycle();
        assert_eq!(outbox.buf.capacity(), kept, "and not reallocated");

        outbox.encode(&numbered(2, 2 * super::RETAINED_CAPACITY));
        assert!(outbox.buf.capacity() > super::RETAINED_CAPACITY);
        outbox.recycle();
        assert!(
            outbox.buf.capacity() <= super::RETAINED_CAPACITY,
            "the excess of an oversized message is given back"
        );
    }

    /// A blocking sink that lets each write through only when the test says
    /// so -- stdout with a peer that has stopped reading, until it hasn't.
    ///
    /// Raises `gone` when dropped, which is when the writer thread that owns
    /// it has exited.
    #[cfg(feature = "server")]
    struct GatedSink {
        sink: SharedSink,
        permits: std::sync::mpsc::Receiver<()>,
        gone: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    #[cfg(feature = "server")]
    impl std::io::Write for GatedSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            // A test that has gone away lets everything through.
            let _ = self.permits.recv();
            self.sink.write(buf)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.sink.flush()
        }
    }

    #[cfg(feature = "server")]
    impl Drop for GatedSink {
        fn drop(&mut self) {
            self.gone.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// A writer thread over a [`GatedSink`], already told to drain `queued`
    /// messages of `pad` bytes each. Returns what the test holds: the sink to
    /// read, the permits to hand out, the drain signal, and the flag the
    /// thread raises on its way out.
    #[cfg(feature = "server")]
    async fn a_gated_writer_thread(
        queued: usize,
        pad: usize,
    ) -> (
        SharedSink,
        std::sync::mpsc::Sender<()>,
        DrainSignal,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        let mut sender = self::queued(queued, pad).await;
        let sink = SharedSink::default();
        let (permit, permits) = std::sync::mpsc::channel();
        let gone = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let gated = GatedSink {
            sink: sink.clone(),
            permits,
            gone: gone.clone(),
        };

        // Cancelled up front: the shutdown has begun, and the thread goes
        // straight to draining what is queued.
        let token = CancellationToken::new();
        token.cancel();
        let (guard, mut drained) = DrainSignal::new();
        let stop = sender
            .start_blocking(gated, token, guard)
            .expect("the thread starts")
            .expect("a fresh sender");
        drained.stop_on_timeout(stop);

        (sink, permit, drained, gone)
    }

    /// The thread's half of the drain contract: it holds its guard until what
    /// was queued is written, so a shutdown that waits for it waits for the
    /// bytes -- not merely for the channel to empty.
    #[cfg(feature = "server")]
    #[tokio::test]
    async fn a_writer_thread_holds_its_guard_until_the_queue_is_written() {
        const QUEUED: usize = 3;

        let (sink, permit, drained, _gone) = a_gated_writer_thread(QUEUED, 0).await;
        let waiting = tokio::spawn(drained.wait_or_abort(std::time::Duration::from_secs(5)));

        // The write is held at the gate, so the wait must be too.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!waiting.is_finished(), "nothing is written yet");

        permit.send(()).expect("the thread is waiting");
        assert!(
            waiting.await.expect("the wait itself does not fail"),
            "the thread raises its signal once the queue is written"
        );
        assert_eq!(sink.lines(), QUEUED);
    }

    /// The thread cannot be aborted, so a spent budget stops it instead: the
    /// write already under way finishes, as it would on the blocking pool
    /// behind an aborted task, but none starts after it -- the rest of the
    /// queue is dropped, and the thread exits.
    #[cfg(feature = "server")]
    #[tokio::test]
    async fn a_writer_thread_past_its_budget_writes_no_more() {
        // Each message a chunk of its own, so the drain is three writes.
        const QUEUED: usize = 3;
        const PAD: usize = super::WRITE_CHUNK;

        let (sink, permit, drained, gone) = a_gated_writer_thread(QUEUED, PAD).await;

        // The first write is held at the gate for longer than the budget.
        assert!(
            !drained
                .wait_or_abort(std::time::Duration::from_millis(20))
                .await,
            "a writer stuck in a write cannot finish inside the budget"
        );
        assert_eq!(sink.lines(), 0, "and has written nothing yet");

        // Let every write through: only the one under way is still made.
        for _ in 0..QUEUED {
            let _ = permit.send(());
        }
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !gone.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("a stopped writer thread exits");
        assert_eq!(
            sink.lines(),
            1,
            "the write under way finishes, and none starts after the budget"
        );
    }

    /// A peer that stops reading leaves the writer stuck in a write nothing
    /// can interrupt. On the runtime's blocking pool, where tokio's stdout
    /// writes, that write held the runtime's drop -- and so the process --
    /// open until the peer read again: a server told to stop never exited. A
    /// thread of neva's own is nobody's to wait for.
    #[cfg(feature = "server")]
    #[test]
    fn a_writer_thread_stuck_in_a_write_does_not_hold_the_runtime_open() {
        let (dropped, runtime_dropped) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("a runtime for the server");
            let permit = runtime.block_on(async {
                let (_sink, permit, drained, _gone) = a_gated_writer_thread(1, 0).await;
                assert!(
                    !drained
                        .wait_or_abort(std::time::Duration::from_millis(20))
                        .await,
                    "the writer is stuck at the gate"
                );
                permit
            });

            // `App::run_blocking` drops its runtime here, with the writer
            // still stuck: the gate stays shut until the drop has returned.
            drop(runtime);
            let _ = dropped.send(());
            drop(permit);
        });

        assert!(
            runtime_dropped
                .recv_timeout(std::time::Duration::from_secs(5))
                .is_ok(),
            "dropping the runtime must not wait for a writer stuck in a write"
        );
    }

    #[tokio::test]
    #[cfg(all(feature = "client", target_os = "windows"))]
    async fn it_tests_handshake() {
        use super::options::StdIoOptions;
        use crate::transport::StdIoClient;
        use tokio_util::sync::CancellationToken;

        let client = StdIoClient::new(StdIoOptions::new(
            "cmd.exe",
            ["/c", "ping", "127.0.0.1", "-t"],
        ));
        let token = CancellationToken::new();
        let (_, _) = client.handshake(token.clone()).unwrap();

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        token.cancel();

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            tokio::process::Command::new("tasklist").output(),
        )
        .await
        .unwrap();

        assert!(
            !String::from_utf8_lossy(&result.unwrap().stdout).contains("ping.exe"),
            "Ping should be terminated"
        );
    }

    #[tokio::test]
    #[cfg(all(feature = "client", target_os = "linux"))]
    async fn it_tests_handshake() {
        use super::options::StdIoOptions;
        use crate::transport::StdIoClient;
        use tokio_util::sync::CancellationToken;

        let client = StdIoClient::new(StdIoOptions::new("sh", ["-c", "sleep 300"]));
        let token = CancellationToken::new();
        let (_, _) = client.handshake(token.clone()).unwrap();

        token.cancel();

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let output = tokio::process::Command::new("pgrep")
            .arg("-f")
            .arg("sleep 300")
            .output()
            .await
            .unwrap();

        assert!(output.stdout.is_empty(), "Process still running");
    }

    /// Regression test for <https://github.com/RomanEmreis/neva/issues/125>,
    /// on every target since <https://github.com/RomanEmreis/neva/issues/128>:
    /// Windows used to route the command through `cmd /c`, so the spawn always
    /// succeeded there and a missing binary surfaced as a child exit long
    /// after `handshake` had returned.
    #[tokio::test]
    #[cfg(feature = "client")]
    async fn it_returns_an_error_instead_of_panicking_when_the_command_cannot_be_spawned() {
        use super::options::StdIoOptions;
        use crate::transport::StdIoClient;
        use tokio_util::sync::CancellationToken;

        const COMMAND: &str = "neva-nonexistent-command-for-issue-125-repro";

        let client = StdIoClient::new(StdIoOptions::new(COMMAND, []));
        let token = CancellationToken::new();

        let result = client.handshake(token);
        let err = match result {
            Err(err) => err,
            Ok(_) => panic!("spawning a nonexistent command should return an Err, not succeed"),
        };
        assert!(
            err.to_string().contains(COMMAND),
            "the error should name the command that failed to spawn, got: {err}"
        );
    }
}

/// The server's stdin is read on a thread of neva's own rather than through
/// [`tokio::io::stdin`], so that a read still parked when the peer goes quiet
/// cannot hold the process open. These pin the behaviour that depends on it.
#[cfg(all(test, feature = "server"))]
mod blocking_reader_tests {
    use super::*;
    use std::time::Duration;

    /// The reader forwards what it parses, on a thread the runtime does not
    /// own -- the whole point being that shutdown never waits for it.
    #[tokio::test]
    async fn it_forwards_lines_read_from_a_blocking_source() {
        let mut receiver = StdIoReceiver::new();
        let (replies, _replies_rx) = mpsc::channel(8);
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n".to_vec();

        receiver.start_blocking(
            std::io::BufReader::new(std::io::Cursor::new(input)),
            replies,
            CancellationToken::new(),
        );

        let msg = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .expect("the reader must forward the line")
            .expect("a readable message");

        let Message::Request(req) = msg else {
            panic!("expected a request");
        };
        assert_eq!(req.method, "tools/list");
    }

    /// An unreadable request is answered rather than dropped or pushed into
    /// the receive loop -- the same routing the async reader does.
    #[tokio::test]
    async fn it_answers_an_unreadable_request_out_of_band() {
        let receiver = StdIoReceiver::new();
        let (replies, mut replies_rx) = mpsc::channel(8);
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":}\n".to_vec();

        receiver.start_blocking(
            std::io::BufReader::new(std::io::Cursor::new(input)),
            replies,
            CancellationToken::new(),
        );

        let reply = tokio::time::timeout(Duration::from_secs(5), replies_rx.recv())
            .await
            .expect("a parse error must be answered")
            .expect("a reply");

        assert!(matches!(reply, Message::Response(_)));
    }

    /// Cancellation is observed between lines, so a host that keeps running
    /// after the server stops is not left with a thread reading into a
    /// channel nobody drains.
    #[tokio::test]
    async fn it_stops_reading_once_cancelled() {
        let mut receiver = StdIoReceiver::new();
        let (replies, _replies_rx) = mpsc::channel(8);
        let token = CancellationToken::new();
        token.cancel();

        receiver.start_blocking(
            std::io::BufReader::new(std::io::Cursor::new(
                b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n".to_vec(),
            )),
            replies,
            token,
        );

        // Nothing is forwarded: the loop checks the token before its first read.
        let got = tokio::time::timeout(Duration::from_millis(300), receiver.recv()).await;
        assert!(got.is_err(), "a cancelled reader must forward nothing");
    }
}

//! stdio transport implementation

use crate::error::{Error, ErrorCode};
use crate::transport::{
    DrainGuard, DrainSignal, Receiver as TransportReceiver, Sender as TransportSender, Transport,
    TransportHandle,
};
use crate::types::Message;
use futures_util::TryFutureExt;
use tokio::{
    io::{AsyncWrite, AsyncWriteExt, BufWriter},
    sync::mpsc::{self, Receiver, Sender},
};

// The async reader serves a child process's piped stdout, which only a client
// has. A server reads its own stdin, and does that on a thread of its own --
// see `StdIoReceiver::start_blocking`.
#[cfg(feature = "client")]
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio_util::sync::CancellationToken;

#[cfg(feature = "server")]
use tokio::io::Stdout;

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

/// Represents stdio server transport
#[cfg(feature = "server")]
pub(crate) struct StdIoServer {
    sender: StdIoSender,
    receiver: StdIoReceiver,
}

/// Represents stdio client transport
#[cfg(feature = "client")]
pub(crate) struct StdIoClient {
    sender: StdIoSender,
    receiver: StdIoReceiver,
    options: StdIoOptions,
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

    /// Starts a new thread that writes to stdout asynchronously
    ///
    /// `drained` is held for as long as this writer may still write, so a
    /// caller awaiting the transport's drain signal knows the queue reached
    /// stdout rather than merely reaching the channel. The task is handed back
    /// so that caller can also end it when the shutdown budget runs out.
    pub(crate) fn start<T: AsyncWrite + Unpin + Send + 'static>(
        &mut self,
        mut writer: BufWriter<T>,
        token: CancellationToken,
        drained: DrainGuard,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let Some(mut receiver) = self.rx.take() else {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "The stdout writer already in use");
            return None;
        };

        Some(tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => break,
                    resp = receiver.recv() => {
                        match resp {
                            Some(resp) => write_message(&mut writer, resp).await,
                            None => return,
                        }
                    }
                }
            }

            // Cancellation stops new work, it does not discard queued work.
            // What sits in the channel here was written by a handler that
            // finished before the teardown got this far -- notably the
            // graceful-close result of a `subscriptions/listen` stream, which
            // the shutdown drain waited for on purpose. `try_recv`, since the
            // senders outlive this task and `recv` would never return `None`.
            while let Ok(resp) = receiver.try_recv() {
                write_message(&mut writer, resp).await;
            }

            // Last thing the writer does: everything queued is now on the
            // wire, so whoever is waiting on the transport to finish may stop
            // waiting. Explicit rather than left to the end of the scope --
            // the drop is the signal.
            drop(drained);
        }))
    }
}

/// Serializes one message as a line of JSON on stdout, flushing it.
///
/// Neither failure is fatal to the writer: a message that cannot be serialized
/// is this message's problem, and a write error on stdout is reported by the
/// next one too.
#[inline]
async fn write_message<T: AsyncWrite + Unpin + Send>(writer: &mut BufWriter<T>, resp: Message) {
    match serde_json::to_vec(&resp) {
        Ok(mut json_bytes) => {
            json_bytes.push(b'\n');
            if let Err(_err) = writer.write_all(&json_bytes).await {
                #[cfg(feature = "tracing")]
                tracing::error!(logger = "neva", "stdout write error: {:?}", _err);
            }
            let _ = writer.flush().await;
        }
        Err(_err) => {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "Serialization error: {:?}", _err);
        }
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
/// A line that isn't a readable `Message` -- malformed JSON, or a JSON-RPC
/// error whose `code` falls outside neva's [`ErrorCode`] set (the TS SDK's
/// `-32000` "server not initialized" family) -- must **not** tear down the
/// receive loop: the peer is alive and every following line is still
/// readable. Pushing a bare `Err` did exactly that, and since the loop
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
        }
    }

    /// Handshakes stdio between client and server apps
    fn handshake(
        &self,
        token: CancellationToken,
    ) -> (BufReader<ChildStdout>, BufWriter<ChildStdin>) {
        let options = &self.options;
        #[cfg(target_os = "linux")]
        let (job, mut child) =
            linux::Job::new(options.command, &options.args).expect("Failed to handshake");
        #[cfg(target_os = "windows")]
        let (job, mut child) =
            windows::Job::new(options.command, &options.args).expect("Failed to handshake");
        #[cfg(all(not(target_os = "windows"), not(target_os = "linux")))]
        let mut child = tokio::process::Command::new(options.command)
            .args(&options.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("Failed to handshake");

        let stdin = child
            .stdin
            .take()
            .expect("Failed to handshake: Inaccessible stdin");
        let stdout = child
            .stdout
            .take()
            .expect("Failed to handshake: Inaccessible stdout");

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

        (BufReader::new(stdout), BufWriter::new(stdin))
    }
}

#[cfg(feature = "server")]
impl StdIoServer {
    /// Creates a new stdio transport for server
    pub(crate) fn new() -> Self {
        Self {
            receiver: StdIoReceiver::new(),
            sender: StdIoSender::new(),
        }
    }

    /// Initializes and Returns handles to `stdin` and `stdout`.
    ///
    /// `stdin` is the std one, read on a thread of our own: see
    /// [`StdIoReceiver::start_blocking`] for why tokio's cannot be used here.
    /// `stdout` stays async -- a write parks on the blocking pool only for as
    /// long as the write takes, so it is never the thing left outstanding at
    /// shutdown.
    pub(crate) fn init() -> (std::io::BufReader<std::io::Stdin>, BufWriter<Stdout>) {
        (
            std::io::BufReader::new(std::io::stdin()),
            BufWriter::new(tokio::io::stdout()),
        )
    }
}

impl TransportSender for StdIoSender {
    async fn send(&mut self, msg: Message) -> Result<(), Error> {
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

#[cfg(feature = "client")]
impl Transport for StdIoClient {
    type Sender = StdIoSender;
    type Receiver = StdIoReceiver;

    fn start(&mut self) -> TransportHandle {
        let token = CancellationToken::new();
        let (reader, writer) = self.handshake(token.clone());
        let (guard, mut drained) = DrainSignal::new();

        self.receiver
            .start(reader, self.sender.tx.clone(), token.clone());

        if let Some(task) = self.sender.start(writer, token.clone(), guard) {
            drained.abort_on_timeout(task.abort_handle());
        }

        #[cfg(feature = "tracing")]
        tracing::info!(logger = "neva", "Connected: stdio");
        TransportHandle::new(token, drained)
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

    fn start(&mut self) -> TransportHandle {
        let token = CancellationToken::new();
        let (reader, writer) = StdIoServer::init();
        // Only the writer takes a guard: the reader is a thread of neva's own
        // (see `start_blocking`), parked in a read nothing can interrupt, and
        // shutdown was never allowed to wait for it -- nor can it be aborted,
        // being nothing the runtime owns.
        let (guard, mut drained) = DrainSignal::new();

        self.receiver
            .start_blocking(reader, self.sender.tx.clone(), token.clone());

        if let Some(task) = self.sender.start(writer, token.clone(), guard) {
            drained.abort_on_timeout(task.abort_handle());
        }

        #[cfg(feature = "tracing")]
        tracing::info!(logger = "neva", "Listening: stdio");
        TransportHandle::new(token, drained)
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

    /// A JSON-RPC error whose code is outside neva's `ErrorCode` set (the
    /// TS SDK's `-32000`) is the exact reply a legacy server sends to the
    /// 2026-07-28 client's pre-initialize `server/discover`. It must complete the
    /// pending request as an id-bound `ParseError`, not kill the loop.
    #[test]
    fn unknown_error_code_becomes_an_id_bound_parse_error() {
        let line = r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32000,"message":"Server not initialized"}}"#;

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
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"Server not initialized"}}"#,
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
            panic!("the legacy rejection must complete the pending request");
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
    /// A sink the test can read back while the writer task owns it.
    #[derive(Clone, Default)]
    struct SharedSink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl SharedSink {
        /// How many complete lines have been written so far.
        fn lines(&self) -> usize {
            match self.0.lock() {
                Ok(buf) => buf.iter().filter(|b| **b == b'\n').count(),
                Err(_) => 0,
            }
        }
    }

    impl tokio::io::AsyncWrite for SharedSink {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            if let Ok(mut sink) = self.get_mut().0.lock() {
                sink.extend_from_slice(buf);
            }
            std::task::Poll::Ready(Ok(buf.len()))
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

    /// Shutdown cancels the writer, but whatever a handler already queued still
    /// has to reach stdout -- the graceful-close result of a
    /// `subscriptions/listen` stream is written in exactly that window, after
    /// the drain phase woke the handler and before the transport goes down.
    /// Breaking out of the loop on cancellation without draining is what used
    /// to lose it.
    #[tokio::test]
    async fn a_cancelled_writer_still_flushes_what_was_queued() {
        use super::StdIoSender;
        use crate::transport::DrainSignal;
        use crate::types::{Message, notification::Notification};
        use tokio::io::BufWriter;
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
        sender.start(BufWriter::new(sink.clone()), token, guard);

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
    struct SlowSink {
        sink: SharedSink,
        delay: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
    }

    impl SlowSink {
        /// Long enough that no write completes by accident, short enough that
        /// the whole queue still clears well inside a test's patience.
        const WRITE_DELAY: std::time::Duration = std::time::Duration::from_millis(50);

        fn new(sink: SharedSink) -> Self {
            Self { sink, delay: None }
        }
    }

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
    fn shutdown_on_an_owned_runtime(queued: usize, join: bool) -> usize {
        use super::StdIoSender;
        use crate::transport::DrainSignal;
        use crate::types::{Message, notification::Notification};
        use tokio::io::BufWriter;
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
            sender.start(
                BufWriter::new(SlowSink::new(sink.clone())),
                token.clone(),
                guard,
            );

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
        let (_, _) = client.handshake(token.clone());

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
        let (_, _) = client.handshake(token.clone());

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

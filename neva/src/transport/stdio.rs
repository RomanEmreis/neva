//! stdio transport implementation

use crate::error::{Error, ErrorCode};
use crate::transport::{Receiver as TransportReceiver, Sender as TransportSender, Transport};
use crate::types::Message;
use futures_util::TryFutureExt;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, BufWriter},
    sync::mpsc::{self, Receiver, Sender},
};
use tokio_util::sync::CancellationToken;

#[cfg(feature = "server")]
use tokio::io::{Stdin, Stdout};

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
    pub(crate) fn start<T: AsyncWrite + Unpin + Send + 'static>(
        &mut self,
        mut writer: BufWriter<T>,
        token: CancellationToken,
    ) {
        let Some(mut receiver) = self.rx.take() else {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "The stdout writer already in use");
            return;
        };

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => break,
                    resp = receiver.recv() => {
                        match resp {
                            Some(resp) => {
                                match serde_json::to_vec(&resp) {
                                    Ok(mut json_bytes) => {
                                        json_bytes.push(b'\n');
                                        if let Err(_err) = writer.write_all(&json_bytes).await {
                                            #[cfg(feature = "tracing")]
                                            tracing::error!(
                                                logger = "neva",
                                                "stdout write error: {:?}", _err);
                                        }
                                        let _ = writer.flush().await;
                                    },
                                    Err(_err) => {
                                        #[cfg(feature = "tracing")]
                                        tracing::error!(
                                            logger = "neva",
                                            "Serialization error: {:?}", _err);
                                    }
                                }
                            },
                            None => break,
                        }
                    }
                }
            }
        });
    }
}

/// Parses one stdio line into a [`Message`].
///
/// A line that isn't a readable `Message` — malformed JSON, or a JSON-RPC
/// error whose `code` falls outside neva's [`ErrorCode`] set (the TS SDK's
/// `-32000` "server not initialized" family) — must **not** tear down the
/// receive loop: the peer is alive and every following line is still
/// readable. Pushing a bare `Err` did exactly that, and since the loop
/// died before completing the pending request, the caller saw a timeout
/// instead of the parse failure.
///
/// So when the line carries an `id`, the failure is reported as an
/// id-bound `ParseError` response: the pending request completes with the
/// real cause. That is also what makes the RC client's dual-mode fallback
/// reachable over stdio — it classifies such a rejection as "legacy peer"
/// and retries `initialize`, which a timeout never could.
///
/// A line with no usable `id` belongs to no pending request; it is logged
/// and dropped.
fn parse_line(line: &str) -> Option<Message> {
    let err = match serde_json::from_str::<Message>(line) {
        Ok(msg) => return Some(msg),
        Err(err) => err,
    };

    let id = serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|line| line.get("id").cloned())
        .and_then(|id| serde_json::from_value::<crate::types::RequestId>(id).ok());

    let Some(id) = id else {
        #[cfg(feature = "tracing")]
        tracing::error!(
            logger = "neva",
            "Failed to parse an unaddressed stdio message: {}",
            err
        );
        return None;
    };

    Some(Message::Response(crate::types::Response::error(
        id,
        Error::new(ErrorCode::ParseError, err.to_string()),
    )))
}

impl StdIoReceiver {
    /// Creates a new stdio transport receiver
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::channel(100);
        Self { tx, rx }
    }

    /// Starts a new thread that reads from stdin asynchronously
    pub(crate) fn start<T: AsyncRead + Unpin + Send + 'static>(
        &self,
        mut reader: BufReader<T>,
        token: CancellationToken,
    ) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut line = String::new();
            loop {
                line.clear();
                tokio::select! {
                    biased;
                    _ = token.cancelled() => break,
                    read_line = reader.read_line(&mut line) => {
                        match read_line {
                            Ok(0) => break, // EOF
                            Ok(_) => {
                                let Some(msg) = parse_line(&line) else { continue };
                                if let Err(_e) = tx.send(Ok(msg)).await {
                                    #[cfg(feature = "tracing")]
                                    tracing::error!(logger = "neva", "Failed to send request: {:?}", _e);
                                    break;
                                }
                            }
                            Err(err) => {
                                let err = Err(err.into());
                                if let Err(_e) = tx.send(err).await {
                                    #[cfg(feature = "tracing")]
                                    tracing::error!(logger = "neva", "Failed to send error request: {:?}", _e);
                                }
                                break;
                            }
                        };
                    }
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

    /// Initializes and Returns references to `stdin` and `stdout`
    pub(crate) fn init() -> (BufReader<Stdin>, BufWriter<Stdout>) {
        (
            BufReader::new(tokio::io::stdin()),
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

    fn start(&mut self) -> CancellationToken {
        let token = CancellationToken::new();
        let (reader, writer) = self.handshake(token.clone());

        self.receiver.start(reader, token.clone());
        self.sender.start(writer, token.clone());

        #[cfg(feature = "tracing")]
        tracing::info!(logger = "neva", "Connected: stdio");
        token
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

    fn start(&mut self) -> CancellationToken {
        let token = CancellationToken::new();
        let (reader, writer) = StdIoServer::init();

        self.receiver.start(reader, token.clone());
        self.sender.start(writer, token.clone());

        #[cfg(feature = "tracing")]
        tracing::info!(logger = "neva", "Listening: stdio");
        token
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
    /// RC client's pre-initialize `server/discover`. It must complete the
    /// pending request as an id-bound `ParseError`, not kill the loop.
    #[test]
    fn unknown_error_code_becomes_an_id_bound_parse_error() {
        let line = r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32000,"message":"Server not initialized"}}"#;

        let Some(Message::Response(crate::types::Response::Err(resp))) = parse_line(line) else {
            panic!("an addressed parse failure must produce an error response");
        };
        assert_eq!(resp.id, RequestId::Number(7));
        assert_eq!(resp.error.code, ErrorCode::ParseError);
    }

    /// The regression the fix is really about: the bad line must not end
    /// the stream — the pending request completes *and* the following
    /// lines keep arriving.
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

        let mut receiver = StdIoReceiver::new();
        receiver.start(BufReader::new(INPUT), CancellationToken::new());

        let Ok(Message::Response(crate::types::Response::Err(resp))) = receiver.recv().await else {
            panic!("the legacy rejection must complete the pending request");
        };
        assert_eq!(resp.id, RequestId::Number(1));
        assert_eq!(resp.error.code, ErrorCode::ParseError);

        // The unaddressed garbage line is skipped, not fatal.
        assert!(
            matches!(receiver.recv().await, Ok(Message::Notification(_))),
            "the stream must survive both bad lines"
        );
    }

    #[test]
    fn an_id_is_salvaged_only_when_the_line_is_still_valid_json() {
        let line = r#"{"jsonrpc":"2.0","id":"abc","result":}"#;
        // Broken JSON — the id is salvaged from the raw text only when the
        // document itself still parses, so this one is unaddressable.
        assert!(parse_line(line).is_none());

        // Valid JSON that simply isn't a `Message`.
        let line = r#"{"jsonrpc":"2.0","id":"abc","totally":"unknown"}"#;
        let Some(Message::Response(crate::types::Response::Err(resp))) = parse_line(line) else {
            panic!("an addressed parse failure must produce an error response");
        };
        assert_eq!(resp.id, RequestId::String("abc".into()));
    }

    #[test]
    fn unaddressed_garbage_is_dropped_not_fatal() {
        assert!(parse_line("not json at all").is_none());
        assert!(parse_line(r#"{"jsonrpc":"2.0","method":123}"#).is_none());
    }

    #[test]
    fn well_formed_lines_are_unaffected() {
        let line = r#"{"jsonrpc":"2.0","method":"ping"}"#;
        assert!(matches!(parse_line(line), Some(Message::Notification(_))));
    }
}

#[cfg(test)]
mod tests {
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

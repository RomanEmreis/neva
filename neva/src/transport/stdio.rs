//! stdio transport implementation

use futures_util::TryFutureExt;
use tokio_util::sync::CancellationToken;
use crate::error::{Error, ErrorCode};
use crate::types::Message;
use tokio::{
    io::{
        AsyncWrite, AsyncWriteExt,
        AsyncRead, AsyncBufReadExt,
        BufReader, BufWriter
    },
    sync::mpsc::{self, Receiver, Sender},
};
use crate::transport::{
    Transport, 
    Sender as TransportSender, 
    Receiver as TransportReceiver
};

#[cfg(feature = "server")]
use tokio::io::{Stdin, Stdout};

#[cfg(feature = "client")]
use tokio::process::{ChildStdin, ChildStdout};
#[cfg(feature = "client")]
use self::options::StdIoOptions;

#[cfg(all(feature = "client", target_os = "windows"))]
mod windows;
#[cfg(all(feature = "client", target_os = "linux"))]
mod linux;

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
    rx: Receiver<Result<Message, Error>>
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
        token: CancellationToken
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
        token: CancellationToken
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
                                let req = match serde_json::from_str(&line) {
                                    Ok(req) => Ok(req),
                                    Err(err) => Err(err.into()),
                                };
                                if let Err(_e) = tx.send(req).await {
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
    fn handshake(&self, token: CancellationToken) -> (BufReader<ChildStdout>, BufWriter<ChildStdin>) {
        let options =  &self.options;
        #[cfg(target_os = "linux")]
        let (job, mut child) = linux::Job::new(options.command, &options.args)
            .expect("Failed to handshake");
        #[cfg(target_os = "windows")]
        let (job, mut child) = windows::Job::new(options.command, &options.args)
            .expect("Failed to handshake");
        #[cfg(all(not(target_os = "windows"), not(target_os = "linux")))]
        let mut child = tokio::process::Command::new(options.command)
            .args(options.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("Failed to handshake");

        let stdin = child.stdin
            .take()
            .expect("Failed to handshake: Inaccessible stdin");
        let stdout = child.stdout
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
            sender: StdIoSender::new()
        }
    }

    /// Initializes and Returns references to `stdin` and `stdout`
    pub(crate) fn init() -> (BufReader<Stdin>, BufWriter<Stdout>) {
        (BufReader::new(tokio::io::stdin()), BufWriter::new(tokio::io::stdout()))
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
        self.rx
            .recv()
            .await
            .unwrap_or_else(|| Err(Error::new(ErrorCode::InvalidRequest, "Unexpected end of stream")))
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
mod tests {
    #[tokio::test]
    #[cfg(all(feature = "client", target_os = "windows"))]
    async fn it_tests_handshake() {
        use tokio_util::sync::CancellationToken;
        use crate::transport::StdIoClient;
        use super::options::StdIoOptions;
        
        let client = StdIoClient::new(StdIoOptions::new("cmd.exe", ["/c", "ping", "127.0.0.1", "-t"]));
        let token = CancellationToken::new();
        let (_, _) = client.handshake(token.clone());

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        
        token.cancel();

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            tokio::process::Command::new("tasklist").output()
        ).await.unwrap();

        assert!(
            !String::from_utf8_lossy(&result.unwrap().stdout).contains("ping.exe"),
            "Ping should be terminated"
        );
    }

    #[tokio::test]
    #[cfg(all(feature = "client", target_os = "linux"))]
    async fn it_tests_handshake() {
        //use tokio::io::AsyncBufReadExt;
        use tokio_util::sync::CancellationToken;
        use crate::transport::StdIoClient;
        use super::options::StdIoOptions;

        let client = StdIoClient::new(StdIoOptions::new("sh", ["-c", "sleep 300"]));
        let token = CancellationToken::new();
        let (_, _) = StdIo::handshake(token.clone());

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

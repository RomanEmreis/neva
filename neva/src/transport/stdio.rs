use futures_util::TryFutureExt;
use crate::{
    error::{Error, ErrorCode},
    types::{Request, Response}
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::mpsc::{Receiver, Sender},
};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use crate::transport::{
    Transport, 
    Sender as TransportSender, 
    Receiver as TransportReceiver
};

/// Represents stdio transport
pub struct StdIo {
    sender: StdIoSender,
    receiver: StdIoReceiver,
}

/// Represents stdio sender
pub struct StdIoSender {
    tx: Sender<Response>,
    rx: Option<Receiver<Response>>,
}

/// Represents stdio receiver
pub struct StdIoReceiver {
    tx: Sender<Result<Request, Error>>,
    rx: Receiver<Result<Request, Error>>
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
    pub(crate) fn start(&mut self) {
        let Some(mut receiver) = self.rx.take() else {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "The stdout writer already in use");
            return;
        };
        
        tokio::spawn(async move {
            let mut stdout = tokio::io::stdout();
            while let Some(resp) = receiver.recv().await {
                match serde_json::to_vec(&resp) {
                    Ok(mut json_bytes) => {
                        json_bytes.push(b'\n');
                        if let Err(_err) = stdout.write_all(&json_bytes).await {
                            #[cfg(feature = "tracing")]
                            tracing::error!(logger = "neva", "stdout write error: {:?}", _err);
                        }
                        let _ = stdout.flush().await;
                    },
                    Err(_err) => {
                        #[cfg(feature = "tracing")]
                        tracing::error!(logger = "neva", "Serialization error: {:?}", _err);
                    }
                };
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
    pub(crate) fn start(&self) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let stdin = tokio::io::stdin();
            let mut reader = BufReader::new(stdin);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        let req = match serde_json::from_str::<Request>(&line) {
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
        });
    }
}

impl StdIo {
    /// Creates a new stdio transport
    pub(crate) fn new() -> Self {
        Self { 
            receiver: StdIoReceiver::new(),
            sender: StdIoSender::new(), 
        }
    }
}

impl TransportSender for StdIoSender {
    async fn send(&mut self, resp: Response) -> Result<(), Error> {
        self.tx
            .send(resp)
            .map_err(|err| Error::new(ErrorCode::InternalError, err))
            .await
    }
}

impl TransportReceiver for StdIoReceiver {
    async fn recv(&mut self) -> Result<Request, Error> {
        match self.rx.recv().await {
            Some(res) => Ok(res?),
            None => Err(Error::new(ErrorCode::InvalidRequest, "Unexpected end of stream"))
        }
    }
}

impl Transport for StdIo {
    type Sender = StdIoSender;
    type Receiver = StdIoReceiver;
    
    fn start(&mut self) {
        self.receiver.start();
        self.sender.start();

        #[cfg(feature = "tracing")]
        tracing::info!(logger = "neva", "Listening: stdio");
    }

    fn split(self) -> (Self::Sender, Self::Receiver) {
        (self.sender, self.receiver)
    }
}

#[cfg(test)]
mod tests {
    
}
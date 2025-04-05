use crate::{
    error::{Error, ErrorCode},
    types::{Request, Response}
};
use tokio::{
    io::{AsyncBufReadExt, BufReader, Stdout},
    sync::mpsc::{Receiver, Sender},
};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use crate::transport::Transport;

/// Represents stdio transport
pub struct StdIo {
    stdout: Stdout,
    tx: Sender<Result<Request, Error>>,
    rx: Receiver<Result<Request, Error>>
}

impl StdIo {
    /// Creates a new stdio transport
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::channel(100);
        let stdout = tokio::io::stdout();
        Self { stdout, tx, rx }
    }
}

impl Transport for StdIo {
    fn start(&self) {
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        let tx = self.tx.clone();
        tokio::spawn(async move {
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
                        tx.send(req).await.unwrap();
                    }
                    Err(err) => {
                        let err = Err(err.into());
                        tx.send(err).await.unwrap();
                        break;
                    }
                };
            }
        });
    }
    
    async fn recv(&mut self) -> Result<Request, Error> {
        match self.rx.recv().await {
            Some(res) => Ok(res?),
            None => Err(Error::new(ErrorCode::InvalidRequest, "unexpected end of stream"))
        }
    }
    
    async fn send(&mut self, resp: Response) -> Result<(), Error> {
        let mut json_bytes = serde_json::to_vec(&resp)?;
        json_bytes.push(b'\n');
        self.stdout.write_all(json_bytes.as_slice()).await?;
        self.stdout.flush().await?;
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    
}
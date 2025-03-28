use crate::{
    //error::Error,
    types::{Request, Response}
};
use tokio::{
    sync::broadcast::Receiver,
    io::{AsyncBufReadExt, Stdout, BufReader}
};
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;

pub struct StdIo {
    stdout: Stdout,
    rx: Receiver<Result<Request, String>>
}

impl StdIo {
    pub(crate) fn start() -> Self {
        let (tx, rx) = broadcast::channel(100);
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        let mut reader = BufReader::new(stdin);

        tokio::spawn(async move {
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        let req = match serde_json::from_str::<Request>(&line) {
                            Ok(req) => Ok(req),
                            Err(err) => Err(err.to_string()),
                        };
                        tx.send(req).unwrap();
                    }
                    Err(err) => {
                        let err = Err(err.to_string());
                        tx.send(err).unwrap();
                        break;
                    }
                };
            }
        });
        
        Self { stdout, rx }
    }
    
    pub(crate) async fn recv(&mut self) -> Result<Request, String> {
        match self.rx.recv().await {
            Ok(res) => Ok(res?),
            Err(err) => Err(err.to_string())
        }
    }
    
    pub(crate) async fn send(&mut self, resp: Response) {
        let mut json_bytes = serde_json::to_vec(&resp).unwrap();
        json_bytes.push(b'\n');
        self.stdout.write_all(json_bytes.as_slice()).await.unwrap();
        self.stdout.flush().await.unwrap();
    }
}

#[cfg(test)]
mod tests {
    
}
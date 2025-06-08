//! HTTP server implementation

use crate::{error::Error, types::Message};
use super::ServiceUrl;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;
use volga::{
    App, Json, ok, status,
    headers::{
        Header,
        Headers,
        custom_headers
    }
};

const MCP_SESSION_ID: &str = "Mcp-Session-Id";
custom_headers! {
    (McpSessionId, MCP_SESSION_ID)
}

pub(super) async fn serve(
    service_url: ServiceUrl,
    recv_tx: mpsc::Sender<Result<Message, Error>>,
    sender_rx: mpsc::Receiver<Message>,
    token: CancellationToken
) {
    let (req_tx, req_rx) = mpsc::channel::<(Message, oneshot::Sender<Message>)>(100);
    tokio::join!(
        dispatch(recv_tx, sender_rx, req_rx, token.clone()),
        handle(service_url, req_tx, token.clone())
    );
}

async fn dispatch(
    recv_tx: mpsc::Sender<Result<Message, Error>>,
    mut sender_rx: mpsc::Receiver<Message>,
    mut req_rx: mpsc::Receiver<(Message, oneshot::Sender<Message>)>,
    token: CancellationToken
) {
    while let Some((msg, resp_tx)) = req_rx.recv().await {
        let recv_tx = recv_tx.clone();
        tokio::spawn(async move {
            if let Err(_e) = recv_tx.send(Ok(msg)).await {
                #[cfg(feature = "tracing")]
                tracing::error!(logger = "neva", "Failed to send request: {:?}", _e);
            }
        });
        let Some(resp) = sender_rx.recv().await else {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "Failed to send response");
            token.cancel();
            return;
        };
        if let Err(_e) = resp_tx.send(resp) {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "Failed to send response: {:?}", _e);
            token.cancel();
        }
    }
}

async fn handle(
    service_url: ServiceUrl,
    req_tx: mpsc::Sender<(Message, oneshot::Sender<Message>)>, 
    token: CancellationToken
) {
    let mut server = App::new()
        .bind(service_url.addr);
    
    server
        .map_err(handle_http_error)
        .map_group(service_url.endpoint)
        .map_get("/", handle_connection)
        .map_post("/", move |msg: Json<Message>, headers: Headers| {
            handle_message(req_tx.clone(), headers, msg.into_inner())
        });
    
    if let Err(_e) = server.run().await {
        #[cfg(feature = "tracing")]
        tracing::error!(logger = "neva", "HTTP Server was shutdown: {:?}", _e);
        token.cancel();
    }
}

async fn handle_connection(id: Header<McpSessionId>) {
    println!("Connected with session-id: {id}");
}

async fn handle_message(
    req_tx: mpsc::Sender<(Message, oneshot::Sender<Message>)>,
    headers: Headers,
    msg: Message
) -> volga::HttpResult {
    if let Message::Notification(_) = msg {
        return status!(202);
    }

    #[cfg(feature = "tracing")]
    let span = {
        let id = headers
            .get(MCP_SESSION_ID)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string();
        tracing::trace_span!("request", mcp_session_id = %id)
    };
    
    //let (log_tx, log_rx) = mpsc::unbounded_channel::<String>();
    let (resp_tx, resp_rx) = oneshot::channel::<Message>();

    let fut = async move {
        req_tx.send((msg, resp_tx))
            .await
            .map_err(sender_error)?;
        resp_rx
            .await
            .map_err(receiver_error)  
    };
    
    #[cfg(feature = "tracing")]
    let fut = fut.instrument(span);
    ok!(fut.await?)
}

async fn handle_http_error(err: volga::error::Error) {
    println!("Error: {:?}", err);
}

#[inline]
fn sender_error(err: mpsc::error::SendError<(Message, oneshot::Sender<Message>)>) -> volga::error::Error {
    volga::error::Error::new("/", err.to_string())
}

#[inline]
fn receiver_error(err: oneshot::error::RecvError) -> volga::error::Error {
    volga::error::Error::new("/", err.to_string())
}
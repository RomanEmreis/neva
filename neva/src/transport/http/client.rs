//! HTTP client implementation

use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use futures_util::{TryStreamExt, StreamExt};
use eventsource_client::{Client, ClientBuilder, ReconnectOptionsBuilder, SSE};
use reqwest::{RequestBuilder, header::{CONTENT_TYPE, ACCEPT}};
use self::mcp_session::McpSession;
use crate::{
    transport::http::{ServiceUrl, get_mcp_session_id, MCP_SESSION_ID},
    types::Message,
    error::{Error, ErrorCode}
};

pub(super) mod mcp_session;

pub(super) async fn connect(
    service_url: ServiceUrl,
    recv_tx: mpsc::Sender<Result<Message, Error>>,
    sender_rx: mpsc::Receiver<Message>,
    token: CancellationToken
) {
    let session = Arc::new(McpSession::new(service_url, token));
    tokio::join!(
        handle_connection(session.clone(), sender_rx, recv_tx.clone()),
        start_sse_connection(session.clone(), recv_tx.clone())
    );
}

async fn handle_connection(
    session: Arc<McpSession>,
    mut sender_rx: mpsc::Receiver<Message>,
    recv_tx: mpsc::Sender<Result<Message, Error>>,
) {
    let client = reqwest::Client::new();
    let token = session.cancellation_token();
    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => return,
            req = sender_rx.recv() => {
                let Some(req) = req else {
                    #[cfg(feature = "tracing")]
                    tracing::error!(logger = "neva", "Unexpected messaging error");
                    break;
                };
                let mut resp = client
                    .post(session.url().as_str().as_ref())
                    .json(&req)
                    .header(CONTENT_TYPE, "application/json")
                    .header(ACCEPT, "application/json, text/event-stream");

                if let Some(session_id) = session.session_id() {
                    resp = resp.header(MCP_SESSION_ID, session_id.to_string())
                }
                
                crate::spawn_fair!(send_request(
                    session.clone(),
                    resp,
                    req,
                    recv_tx.clone()
                ));
            }
        }
    }
}

async fn send_request(
    session: Arc<McpSession>,
    resp: RequestBuilder,
    req: Message,
    resp_tx: mpsc::Sender<Result<Message, Error>>
) {
    let resp = match resp.send().await { 
        Ok(resp) => resp,
        Err(_err) => {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "Failed to send request: {}", _err);
            return;
        }
    };

    if let Message::Notification(_) = &req {
        return;
    }
    
    if !session.has_session_id() {
        if let Some(session_id) = get_mcp_session_id(resp.headers()) { 
            session.set_session_id(session_id);
        }
    }

    if let Message::Request(r) = req {
        if r.method == crate::commands::INIT {
            session.notify_session_initialized();
        }
    }

    let resp = resp.json::<Message>().await;
    if let Err(_err) = resp_tx.send(resp.map_err(Error::from)).await {
        #[cfg(feature = "tracing")]
        tracing::error!(logger = "neva", "Failed to send response: {}", _err);
    }
}

async fn start_sse_connection(
    session: Arc<McpSession>,
    resp_tx: mpsc::Sender<Result<Message, Error>>
) {
    let token = session.cancellation_token();
    tokio::select! {
        biased;
        _ = token.cancelled() => (),
        _ = session.initialized() => {
            tokio::spawn(handle_sse_connection(session.clone(), resp_tx));        
        }
    }
}

async fn handle_sse_connection(
    session: Arc<McpSession>,
    resp_tx: mpsc::Sender<Result<Message, Error>>
) {
    let Ok(mut client) = ClientBuilder::for_url(session.url().as_str().as_ref()) else { 
        #[cfg(feature = "tracing")]
        tracing::error!(logger = "neva", "Failed to create SSE client");
        return;
    };
    
    client = client
        .reconnect(
            ReconnectOptionsBuilder::new(true)
                .retry_initial(true)
                .delay(std::time::Duration::from_secs(5))
                .delay_max(std::time::Duration::from_secs(10))
                .build(),
        );

    if let Some(session_id) = session.session_id() {
        let Ok(with_header) = client.header(MCP_SESSION_ID, &session_id.to_string()) else { 
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "Failed to create SSE client");
            return;
        };
        client = with_header;
    }
    
    let mut stream = client
        .build_http()
        .stream()
        .fuse()
        .map_ok(|event| handle_event(event, &resp_tx))
        .map_err(handle_error);

    let token = session.cancellation_token();
    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => break,
            fut = stream.try_next() => {
                let Ok(Some(fut)) = fut else {
                    #[cfg(feature = "tracing")]
                    tracing::error!(logger = "neva", "Unexpected stream end");
                    break;
                };
                fut.await;
            }
        }
    }
}

async fn handle_event(event: SSE, resp_tx: &mpsc::Sender<Result<Message, Error>>) {
    match event {
        SSE::Connected(_) => handle_connected(),
        SSE::Comment(comment) => handle_comment(comment),
        SSE::Event(event) => handle_msg(event, resp_tx).await,
    }
}

#[inline]
fn handle_error(_err: eventsource_client::Error) {
    #[cfg(feature = "tracing")]
    tracing::error!(logger = "neva", "SSE Error: {}", _err);
}

#[inline]
fn handle_connected() {
    #[cfg(feature = "tracing")]
    tracing::trace!(logger = "neva", "SSE Connection opened");
}

#[inline]
fn handle_comment(_comment: String) {
    #[cfg(feature = "tracing")]
    tracing::trace!(logger = "neva", "Received a comment: {}", _comment);
}

#[inline]
async fn handle_msg(event: eventsource_client::Event, resp_tx: &mpsc::Sender<Result<Message, Error>>) {
    let msg = serde_json::from_str::<Message>(&event.data);
    if let Err(_err) = resp_tx.send(msg.map_err(Error::from)).await {
        #[cfg(feature = "tracing")]
        tracing::error!(logger = "neva", "Failed to send server request: {}", _err);
    }
}

impl From<reqwest::Error> for Error {
    #[inline]
    fn from(err: reqwest::Error) -> Self {
        Error::new(ErrorCode::ParseError, err.to_string())   
    }
}

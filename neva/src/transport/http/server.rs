//! HTTP server implementation

use std::sync::Arc;
use dashmap::DashMap;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use super::ServiceUrl;
use crate::{error::Error, types::{Message}};
use crate::types::RequestId;
use volga::{
    App, Json, HttpResult, di::Dc, status, ok, 
    http::sse::Message as SseMessage, sse,
    headers::Headers
};

#[cfg(feature = "tracing")]
use crate::types::notification::fmt::{register_log_sender, unregister_log_sender, send};

const MCP_SESSION_ID: &str = "Mcp-Session-Id";

type RequestMap = Arc<DashMap<RequestId, oneshot::Sender<Message>>>;

#[derive(Clone)]
struct RequestManager {
    pending: RequestMap,
    sender: mpsc::Sender<Result<Message, Error>>,
}

impl Default for RequestManager {
    fn default() -> Self {
        unreachable!()
    }
}

pub(super) async fn serve(
    service_url: ServiceUrl,
    recv_tx: mpsc::Sender<Result<Message, Error>>,
    sender_rx: mpsc::Receiver<Message>,
    token: CancellationToken
) {
    let pending = Arc::new(DashMap::new());
    let manager = RequestManager {
        pending: pending.clone(),
        sender: recv_tx,
    };
    tokio::join!(
        dispatch(pending.clone(), sender_rx, token.clone()),
        handle(service_url, manager, token.clone())
    );
}

async fn dispatch(
    pending: RequestMap,
    mut sender_rx: mpsc::Receiver<Message>,
    token: CancellationToken
) {
    while let Some(msg) = sender_rx.recv().await {
        if let Some((_, resp_tx)) = pending.remove(&msg.id()) {
            if let Err(_e) = resp_tx.send(msg) {
                #[cfg(feature = "tracing")]
                tracing::error!(logger = "neva", "Failed to send response: {:?}", _e);
                token.cancel();
            }
        } else {
            send(msg);
        }
    }
}

async fn handle(
    service_url: ServiceUrl,
    manager: RequestManager,
    token: CancellationToken
) {
    let root = "/";
    let mut server = App::new()
        .bind(service_url.addr);
    
    server
        .add_singleton(manager)
        .map_err(handle_http_error)
        .map_group(service_url.endpoint)
        .map_delete(root, handle_session_end)
        .map_get(root, handle_connection)
        .map_post(root, handle_message);
    
    if let Err(_e) = server.run().await {
        #[cfg(feature = "tracing")]
        tracing::error!(logger = "neva", "HTTP Server was shutdown: {:?}", _e);
        token.cancel();
    }
}

async fn handle_session_end(headers: Headers) {
    let Some(id) = headers
        .get(MCP_SESSION_ID)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string()) else {
        return;
    };
    #[cfg(feature = "tracing")]
    unregister_log_sender(&id);
}

async fn handle_connection(headers: Headers) -> HttpResult {
    let Some(id) = headers
        .get(MCP_SESSION_ID)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string()) else { 
        return status!(405);
    };

    let (log_tx, log_rx) = mpsc::unbounded_channel::<Message>();
    #[cfg(feature = "tracing")]
    register_log_sender(id.clone(), log_tx);
    
    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(log_rx)
        .map(handle_sse_message);
    
    sse!(stream, [
        (MCP_SESSION_ID, id)
    ])
}

async fn handle_message(
    manager: Dc<RequestManager>,
    headers: Headers,
    Json(msg): Json<Message>
) -> HttpResult {
    if let Message::Notification(_) = msg {
        return status!(202);
    }
    let id = get_or_create_mcp_session(headers);
    
    let (resp_tx, resp_rx) = oneshot::channel::<Message>();
    manager.pending.insert(msg.id(), resp_tx);
    manager.sender.send(Ok(msg.set_session_id(id.clone())))
        .await
        .map_err(sender_error)?;
    let resp = resp_rx
        .await
        .map_err(receiver_error)?;
    
    ok!(resp, [
        (MCP_SESSION_ID, id)
    ])
}

async fn handle_http_error(err: volga::error::Error) {
    println!("Error: {:?}", err);
}

fn sender_error(err: mpsc::error::SendError<Result<Message, Error>>) -> volga::error::Error {
    volga::error::Error::new("/", err.to_string())
}

fn receiver_error(err: oneshot::error::RecvError) -> volga::error::Error {
    volga::error::Error::new("/", err.to_string())
}

fn handle_sse_message(msg: Message) -> Result<SseMessage, volga::error::Error> {
    Ok(SseMessage::new()
        .id(uuid::Uuid::new_v4())
        .json(msg))
}

#[inline]
fn get_or_create_mcp_session(headers: Headers) -> String {
    headers
        .get(MCP_SESSION_ID)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}

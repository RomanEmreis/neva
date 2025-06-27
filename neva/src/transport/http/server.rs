//! HTTP server implementation

use std::sync::Arc;
use dashmap::DashMap;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::{StreamExt, wrappers::UnboundedReceiverStream};
use tokio_util::sync::CancellationToken;
use super::{ServiceUrl, MCP_SESSION_ID, get_mcp_session_id};
use crate::{
    shared::message_registry::MessageRegistry,
    types::{RequestId, Message},
    error::Error
};
use volga::{
    App, Json, HttpResult, di::Dc, status, ok, 
    http::sse::Message as SseMessage, sse,
    headers::Headers
};

#[cfg(feature = "tracing")]
use crate::types::notification::fmt::LOG_REGISTRY;

type RequestMap = Arc<DashMap<RequestId, oneshot::Sender<Message>>>;

#[derive(Clone)]
struct RequestManager {
    pending: RequestMap,
    msg_registry: Arc<MessageRegistry>,
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
    let registry = Arc::new(MessageRegistry::new());
    let manager = RequestManager {
        pending: pending.clone(),
        msg_registry: registry.clone(),
        sender: recv_tx,
    };
    tokio::join!(
        dispatch(pending.clone(), registry.clone(), sender_rx, token.clone()),
        handle(service_url, manager, token.clone())
    );
}

async fn dispatch(
    pending: RequestMap,
    msg_registry: Arc<MessageRegistry>,
    mut sender_rx: mpsc::Receiver<Message>,
    token: CancellationToken
) {
    while let Some(msg) = sender_rx.recv().await {
        if let Some((_, resp_tx)) = pending.remove(&msg.full_id()) {
            if let Err(_e) = resp_tx.send(msg) {
                #[cfg(feature = "tracing")]
                tracing::error!(logger = "neva", "Failed to send response: {:?}", _e);
                token.cancel();
            }
        } else if let Err(_e) = msg_registry.send(msg) {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "Failed to send server request: {:?}", _e);
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
        .map_get(root, handle_connection)
        .map_post(root, handle_message)
        .map_delete(root, handle_session_end);
    
    if let Err(_e) = server.run().await {
        #[cfg(feature = "tracing")]
        tracing::error!(logger = "neva", "HTTP Server was shutdown: {:?}", _e);
        token.cancel();
    }
}

async fn handle_session_end(manager: Dc<RequestManager>, headers: Headers) -> HttpResult {
    let Some(id) = get_mcp_session_id(&headers) else {
        return status!(405);
    };
    
    #[cfg(feature = "tracing")]
    LOG_REGISTRY.unregister(&id);
    manager.msg_registry.unregister(&id);
    
    ok!([
        (MCP_SESSION_ID, id.to_string())
    ])
}

async fn handle_connection(manager: Dc<RequestManager>, headers: Headers) -> HttpResult {
    let Some(id) = get_mcp_session_id(&headers) else { 
        return status!(405);
    };

    let (log_tx, log_rx) = mpsc::unbounded_channel::<Message>();
    let (msg_tx, msg_rx) = mpsc::unbounded_channel::<Message>();
    
    #[cfg(feature = "tracing")]
    LOG_REGISTRY.register(id, log_tx);
    manager.msg_registry.register(id, msg_tx);
    
    let stream = futures_util::stream::select(
        UnboundedReceiverStream::new(log_rx), 
        UnboundedReceiverStream::new(msg_rx));
    
    sse!(stream.map(handle_sse_message), [
        (MCP_SESSION_ID, id.to_string())
    ])
}

async fn handle_message(
    manager: Dc<RequestManager>,
    headers: Headers,
    Json(msg): Json<Message>
) -> HttpResult {
    let id = get_or_create_mcp_session(headers);
    if let Message::Notification(_) = msg {
        return status!(202, [
            (MCP_SESSION_ID, id.to_string())
        ]);
    }
    let msg = msg.set_session_id(id);
    let (resp_tx, resp_rx) = oneshot::channel::<Message>();
    manager.pending.insert(msg.full_id(), resp_tx);
    manager.sender.send(Ok(msg))
        .await
        .map_err(sender_error)?;
    let resp = resp_rx
        .await
        .map_err(receiver_error)?;
    
    ok!(resp, [
        (MCP_SESSION_ID, id.to_string())
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
        .id(uuid::Uuid::new_v4().to_string())
        .json(msg))
}

#[inline]
fn get_or_create_mcp_session(headers: Headers) -> uuid::Uuid {
    get_mcp_session_id(&headers).unwrap_or_else(uuid::Uuid::new_v4)
}

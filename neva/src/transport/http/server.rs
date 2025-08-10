//! HTTP server implementation

use std::sync::Arc;
use dashmap::DashMap;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::{StreamExt, wrappers::UnboundedReceiverStream};
use tokio_util::sync::CancellationToken;
use super::{HttpRuntimeContext, ServiceUrl, MCP_SESSION_ID, get_mcp_session_id};
use crate::{
    shared::message_registry::MessageRegistry,
    types::{RequestId, Message},
    error::Error
};
use volga::{
    App, Json, HttpResult, di::Dc, status, ok,
    auth::{BearerTokenService, Bearer},
    http::sse::Message as SseMessage, sse,
    headers::{HttpHeaders, AUTHORIZATION}
};

#[cfg(feature = "tracing")]
use crate::types::notification::fmt::LOG_REGISTRY;

pub use auth_config::{AuthConfig, DefaultClaims};

pub mod auth_config;

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
    rt: HttpRuntimeContext,
    token: CancellationToken
) {
    let pending = Arc::new(DashMap::new());
    let registry = Arc::new(MessageRegistry::new());
    let manager = RequestManager {
        pending: pending.clone(),
        msg_registry: registry.clone(),
        sender: rt.tx,
    };
    tokio::join!(
        dispatch(pending.clone(), registry.clone(), rt.rx, token.clone()),
        handle(rt.url, rt.auth, manager, token.clone())
    );
}

async fn dispatch(
    pending: RequestMap,
    msg_registry: Arc<MessageRegistry>,
    mut sender_rx: mpsc::Receiver<Message>,
    token: CancellationToken
) {
    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => break,
            Some(msg) = sender_rx.recv() => {
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
    }
}

async fn handle(
    service_url: ServiceUrl,
    auth: Option<AuthConfig>,
    manager: RequestManager,
    token: CancellationToken
) {
    let root = "/";
    let mut server = App::new()
        .bind(service_url.addr)
        .with_no_delay()
        .without_greeter()
        .with_bearer_auth(|auth| auth);
    
    if let Some(auth) = auth {
        let (auth, rules) = auth.into_parts();
        server = server.with_bearer_auth(|_| auth);
        server.authorize(rules);
    }
    
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

async fn handle_session_end(manager: Dc<RequestManager>, headers: HttpHeaders) -> HttpResult {
    let Some(id) = get_mcp_session_id(&headers) else {
        return status!(405);
    };
    
    #[cfg(feature = "tracing")]
    LOG_REGISTRY.unregister(&id);
    manager.msg_registry.unregister(&id);
    
    ok!([(MCP_SESSION_ID, id.to_string())])
}

async fn handle_connection(manager: Dc<RequestManager>, headers: HttpHeaders) -> HttpResult {
    let Some(id) = get_mcp_session_id(&headers) else { 
        return status!(405);
    };

    let (_log_tx, log_rx) = mpsc::unbounded_channel::<Message>();
    let (msg_tx, msg_rx) = mpsc::unbounded_channel::<Message>();
    
    #[cfg(feature = "tracing")]
    LOG_REGISTRY.register(id, _log_tx);
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
    mut headers: HttpHeaders,
    bearer: Bearer,
    bts: BearerTokenService,
    Json(msg): Json<Message>
) -> HttpResult {
    let id = get_or_create_mcp_session(&headers);
    if let Message::Notification(_) = msg {
        return status!(202, [
            (MCP_SESSION_ID, id.to_string())
        ]);
    }

    headers.remove(AUTHORIZATION);
    
    let msg = msg
        .set_session_id(id)
        .set_headers(headers.into_inner());

    let msg = if let Ok(claims) = bts.decode::<DefaultClaims>(bearer) {
        msg.set_claims(claims)
    } else { 
        msg
    };
    
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

async fn handle_http_error(_err: volga::error::Error) {
    #[cfg(feature = "tracing")]
    tracing::error!(logger = "neva", "HTTP error: {:?}", _err)
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

/// Fetches the [`MCP_SESSION_ID`] header value or created the new [`uuid::Uuid`] if `None`
#[inline]
fn get_or_create_mcp_session(headers: &HttpHeaders) -> uuid::Uuid {
    get_mcp_session_id(headers).unwrap_or_else(uuid::Uuid::new_v4)
}

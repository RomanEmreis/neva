//! HTTP client implementation

use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use futures_util::{TryStreamExt, StreamExt};
use reqwest::{RequestBuilder, header::{CONTENT_TYPE, CACHE_CONTROL, ACCEPT}};
use self::mcp_session::McpSession;
use crate::{
    transport::http::{ClientRuntimeContext, get_mcp_session_id, MCP_SESSION_ID},
    types::Message,
    error::{Error, ErrorCode}
};
#[cfg(feature = "client-tls")]
use tls_config::ClientTlsConfig;

pub(super) mod mcp_session;
#[cfg(feature = "client-tls")]
pub(crate) mod tls_config;

pub(super) async fn connect(
    rt: ClientRuntimeContext,
    token: CancellationToken
) {
    let session = Arc::new(McpSession::new(rt.url, token));
    let access_token: Option<Arc<[u8]>> = rt.access_token.map(|t| t.into());
    tokio::join!(
        handle_connection(
            session.clone(), 
            rt.rx, 
            rt.tx.clone(), 
            access_token.clone(),
            #[cfg(feature = "client-tls")]
            rt.tls_config.clone()
        ),
        start_sse_connection(
            session.clone(), 
            rt.tx.clone(), 
            access_token.clone(),
            #[cfg(feature = "client-tls")]
            rt.tls_config.clone()
        )
    );
}

async fn handle_connection(
    session: Arc<McpSession>,
    mut sender_rx: mpsc::Receiver<Message>,
    recv_tx: mpsc::Sender<Result<Message, Error>>,
    access_token: Option<Arc<[u8]>>,
    #[cfg(feature = "client-tls")]
    tls_config: Option<ClientTlsConfig>,
) {
    #[cfg(not(feature = "client-tls"))]
    let client = match create_client() {
        Ok(client) => client,
        Err(_err) => {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "HTTP client error: {_err:#}");
            return;
        }
    };

    #[cfg(feature = "client-tls")]
    let client = match create_client(tls_config) {
        Ok(client) => client,
        Err(_err) => {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "HTTP client error: {_err:#}");
            return;
        }
    };
    
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
                
                if let Some(access_token) = &access_token {
                    resp = resp.bearer_auth(String::from_utf8_lossy(access_token))
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
            tracing::error!(logger = "neva", "Failed to send HTTP request: {}", _err);
            return;
        }
    };

    if let Message::Notification(_) = &req {
        return;
    }
    
    if !session.has_session_id()
        && let Some(session_id) = get_mcp_session_id(resp.headers()) { 
        session.set_session_id(session_id);
    }

    if let Message::Request(r) = req
        && r.method == crate::commands::INIT {
        session.notify_session_initialized();
        session.sse_ready().await;
    }

    let resp = resp.json::<Message>().await;
    
    if let Err(_err) = resp_tx.send(resp.map_err(Error::from)).await {
        #[cfg(feature = "tracing")]
        tracing::error!(logger = "neva", "Failed to send response: {}", _err);
    }
}

async fn start_sse_connection(
    session: Arc<McpSession>,
    resp_tx: mpsc::Sender<Result<Message, Error>>,
    access_token: Option<Arc<[u8]>>,
    #[cfg(feature = "client-tls")]
    tls_config: Option<ClientTlsConfig>,
) {
    let token = session.cancellation_token();
    tokio::select! {
        biased;
        _ = token.cancelled() => (),
        _ = session.initialized() => {
            tokio::spawn(handle_sse_connection(
                session.clone(), 
                resp_tx, 
                access_token,
                #[cfg(feature = "client-tls")]
                tls_config
            ));        
        }
    }
}

async fn handle_sse_connection(
    session: Arc<McpSession>,
    resp_tx: mpsc::Sender<Result<Message, Error>>,
    access_token: Option<Arc<[u8]>>,
    #[cfg(feature = "client-tls")]
    tls_config: Option<ClientTlsConfig>,
) {
    #[cfg(not(feature = "client-tls"))]
    let client = match create_client() {
        Ok(client) => client,
        Err(_err) => {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "SSE client error: {_err:#}");
            return;
        }
    };
    
    #[cfg(feature = "client-tls")]
    let client = match create_client(tls_config) {
        Ok(client) => client,
        Err(_err) => {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "SSE client error: {_err:#}");
            return;
        }
    };
    
    let mut resp = client
        .get(session.url().as_str().as_ref())
        .header(ACCEPT, "application/json, text/event-stream")
        .header(CACHE_CONTROL, "no-cache");

    if let Some(access_token) = access_token {
        resp = resp.bearer_auth(String::from_utf8_lossy(&access_token));
    }

    if let Some(session_id) = session.session_id() {
        resp = resp.header(MCP_SESSION_ID, session_id.to_string());
    }

    let resp = match resp.send().await {
        Ok(resp) => resp,
        Err(_err) => {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "Failed to send SSE request: {}", _err);
            return;
        }
    };
    
    let mut stream = sse_stream::SseStream::from_byte_stream(resp.bytes_stream())
        .fuse()
        .map_ok(|event| handle_event(event, &resp_tx))
        .map_err(handle_error);
    
    session.notify_sse_initialized();
    
    let token = session.cancellation_token();
    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => break,
            fut = stream.next() => {
                let Some(Ok(fut)) = fut else {
                    #[cfg(feature = "tracing")]
                    tracing::error!(logger = "neva", "Unexpected stream end");
                    break;
                };
                fut.await;
            }
        }
    }
}

async fn handle_event(event: sse_stream::Sse, resp_tx: &mpsc::Sender<Result<Message, Error>>) {
    if event.is_message() {
        handle_msg(event, resp_tx).await
    } else { 
        #[cfg(feature = "tracing")]
        tracing::debug!(logger = "neva", event = ?event);
    }
}

#[inline]
fn handle_error(_err: sse_stream::Error) {
    #[cfg(feature = "tracing")]
    tracing::error!(logger = "neva", "SSE Error: {}", _err);
}

#[inline]
async fn handle_msg(event: sse_stream::Sse, resp_tx: &mpsc::Sender<Result<Message, Error>>) {
    let msg = serde_json::from_str::<Message>(&event.data.unwrap());
    if let Err(_err) = resp_tx.send(msg.map_err(Error::from)).await {
        #[cfg(feature = "tracing")]
        tracing::error!(logger = "neva", "Failed to send server request: {}", _err);
    }
}

#[inline]
#[cfg(not(feature = "client-tls"))]
fn create_client() -> Result<reqwest::Client, Error> {
    reqwest::Client::builder()
        .build()
        .map_err(Error::from)
}

#[inline]
#[cfg(feature = "client-tls")]
fn create_client(mut tls_config: Option<ClientTlsConfig>) -> Result<reqwest::Client, Error> {
    let mut builder = reqwest::ClientBuilder::new();
    if let Some(ca_cert) = tls_config
        .as_mut()
        .and_then(|tls| tls.ca.take()) {
        builder = builder.add_root_certificate(ca_cert);
    }
    if let Some(identity) = tls_config
        .as_mut()
        .and_then(|tls| tls.identity.take()) {
        builder = builder.identity(identity);
    }
    if tls_config.is_some_and(|tls| !tls.certs_verification) { 
        builder = builder.danger_accept_invalid_certs(true);        
    } 
    builder
        .build()
        .map_err(Error::from)
}

impl From<reqwest::Error> for Error {
    #[inline]
    fn from(err: reqwest::Error) -> Self {
        Error::new(ErrorCode::ParseError, err.to_string())   
    }
}

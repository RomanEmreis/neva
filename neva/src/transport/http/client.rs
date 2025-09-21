//! HTTP client implementation

use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use futures_util::{TryStreamExt, StreamExt};
use eventsource_client::{Client, ClientBuilder, ReconnectOptionsBuilder, SSE};
use reqwest::{RequestBuilder, header::{CONTENT_TYPE, ACCEPT, AUTHORIZATION}};
use self::mcp_session::McpSession;
use crate::{
    transport::http::{ClientRuntimeContext, get_mcp_session_id, MCP_SESSION_ID},
    types::Message,
    error::{Error, ErrorCode}
};

#[cfg(feature = "tls")]
use reqwest::Certificate;

pub(super) mod mcp_session;

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
            #[cfg(feature = "tls")]
            rt.ca_cert.clone()),
        start_sse_connection(
            session.clone(), 
            rt.tx.clone(), 
            access_token.clone(),
            #[cfg(feature = "tls")]
            rt.ca_cert.clone()
        )
    );
}

async fn handle_connection(
    session: Arc<McpSession>,
    mut sender_rx: mpsc::Receiver<Message>,
    recv_tx: mpsc::Sender<Result<Message, Error>>,
    access_token: Option<Arc<[u8]>>,
    #[cfg(feature = "tls")]
    ca_cert: Option<Certificate>,
) {
    #[cfg(feature = "tls")]
    let mut builder = reqwest::Client::builder();
    #[cfg(not(feature = "tls"))]
    let builder = reqwest::Client::builder();
    
    #[cfg(feature = "tls")]
    if let Some(ca_cert) = ca_cert { 
        builder = builder
            .add_root_certificate(ca_cert);
    } 
    
    let client = builder.build()
        .expect("Unable to build HTTP client");
    
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
    resp_tx: mpsc::Sender<Result<Message, Error>>,
    access_token: Option<Arc<[u8]>>,
    #[cfg(feature = "tls")]
    ca_cert: Option<Certificate>,
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
                #[cfg(feature = "tls")]
                ca_cert));        
        }
    }
}

async fn handle_sse_connection(
    session: Arc<McpSession>,
    resp_tx: mpsc::Sender<Result<Message, Error>>,
    access_token: Option<Arc<[u8]>>,
    #[cfg(feature = "tls")]
    ca_cert: Option<Certificate>,
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

    if let Some(access_token) = &access_token  {
        let access_token = format!("Bearer {}", String::from_utf8_lossy(access_token));
        let Ok(with_header) = client.header(AUTHORIZATION.as_str(), &access_token) else {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "Failed to create SSE client");
            return;
        };
        client = with_header;
    }
    
    #[cfg(feature = "tls")]
    let client = if let Some(_ca_cert) = ca_cert  { 
        client.build()
    } else {
        client.build()
    };

    #[cfg(not(feature = "tls"))]
    let client = client.build_http();
    
    let mut stream = client
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

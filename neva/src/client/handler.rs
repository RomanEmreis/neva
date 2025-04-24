//! Request handling utilities

use tokio::sync::{oneshot, Mutex};
use tokio_util::sync::CancellationToken;
use std::{
    collections::HashMap,
    time::Duration,
    sync::{Arc, atomic::{AtomicI64, Ordering}}
};
use tokio::time::timeout;
use crate::{
    transport::{Receiver, Sender, TransportProto, TransportProtoReceiver, TransportProtoSender},
    types::{RequestId, Response}
};
use crate::error::{Error, ErrorCode};
use crate::transport::Transport;
use crate::types::notification::{LogMessage, Notification};
use crate::types::Request;

/// Pending requests data structure
type PendingRequests = Arc<Mutex<HashMap<RequestId, RequestHandle>>>; 

/// Represents a request handle
struct RequestHandle {
    sender: oneshot::Sender<Response>,
    _cancellation_token: CancellationToken
}

pub(super) struct RequestHandler {
    /// Request counter
    counter: AtomicI64,
    
    /// Request timeout
    timeout: Duration,

    /// Pending requests
    pending: PendingRequests,

    /// Current transport sender handle
    sender: TransportProtoSender,
}

impl RequestHandle {
    /// Creates a new [`RequestHandle`]
    pub(super) fn new(sender: oneshot::Sender<Response>) -> Self {
        Self { sender, _cancellation_token: CancellationToken::new() }
    }
    
    /// Sends a [`Response`] to MCP server
    pub(super) fn send(self, resp: Response) {
        match self.sender.send(resp) {
            Ok(_) => (),
            Err(_err) => {
                #[cfg(feature = "tracing")]
                tracing::error!(
                    logger = "neva",
                    "Request handler failed to send response: {:?}", _err);
            }
        };
    }
}

impl RequestHandler {
    /// Creates a new [`RequestHandler`]
    pub(super) fn new(transport: TransportProto, timeout: Duration) -> Self {
        let (tx, rx) = transport.split();
        
        let handler = Self {
            counter: AtomicI64::new(1),
            pending: Arc::new(Mutex::new(HashMap::new())),
            sender: tx,
            timeout,
        };
        
        handler.start(rx)
    }

    /// Returns the next [`RequestId`]
    #[inline]
    pub(super) fn next_id(&self) -> RequestId {
        let id = self.counter.fetch_add(1, Ordering::Relaxed);
        RequestId::Number(id)
    }

    /// Sends a request to MCP server
    #[inline]
    pub(super) async fn send_request(&mut self, request: Request) -> Result<Response, Error> {
        let (tx, rx) = oneshot::channel();
        let id = request.id();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id.clone(), RequestHandle::new(tx));
        }

        self.sender.send(request).await?;

        match timeout(self.timeout, rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => Err(Error::new(ErrorCode::InternalError, "Response channel closed")),
            Err(_) => {
                let mut pending = self.pending.lock().await;
                pending.remove(&id);
                Err(Error::new(ErrorCode::Timeout, "Request timed out"))
            }
        }
    }
    
    /// Sends a notification to MCP server
    #[inline]
    pub(super) async fn send_notification(&mut self, notification: Notification) -> Result<(), Error> {
        self.sender.send(notification).await
    }

    #[inline]
    fn start(self, mut rx: TransportProtoReceiver) -> Self {
        let pending = self.pending.clone();
        tokio::task::spawn(async move {
            while let Ok(msg) = rx.recv::<serde_json::Value>().await {
                if msg.get("id").is_some() {
                    let resp = serde_json::from_value::<Response>(msg).unwrap();
                    let sender = {
                        let mut pending = pending.lock().await;
                        pending.remove(&resp.id)
                    };
                    if let Some(sender) = sender {
                        sender.send(resp);
                    }
                } else {
                    #[cfg(feature = "tracing")]
                    {
                        let log = serde_json::from_value::<Notification>(msg).unwrap();
                        let log = serde_json::from_value::<LogMessage>(log.params.unwrap()).unwrap();
                        log.write();
                    }
                }
            }
        });
        self
    }
}
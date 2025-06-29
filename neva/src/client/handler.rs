//! Request handling utilities

use std::sync::Arc;
use tokio::{
    sync::RwLock, 
    time::timeout
};
use std::{time::Duration, sync::atomic::{AtomicI64, Ordering}};
use crate::{
    client::options::McpOptions,
    error::{Error, ErrorCode},
    shared::RequestQueue,
    transport::{
        Receiver, Sender, 
        Transport, TransportProto, TransportProtoReceiver, TransportProtoSender
    },
    types::{
        IntoResponse, Response, Message, 
        RequestId, Request,
        notification::Notification,
        Root, root::ListRootsResult,
        sampling::SamplingHandler
    }
};
use crate::client::notification_handler::NotificationsHandler;

struct Roots {
    /// Cached list of [`Root`]
    inner: Arc<RwLock<Vec<Root>>>,
    
    /// Notifier for Roots cache updates
    sender: Option<tokio::sync::mpsc::Sender<Vec<Root>>>,
}

pub(super) struct RequestHandler {
    /// Request counter
    counter: AtomicI64,
    
    /// Request timeout
    timeout: Duration,

    /// Pending requests
    pending: RequestQueue,

    /// Current transport sender handle
    sender: TransportProtoSender,
    
    /// Cached list of [`Root`]
    roots: Roots,

    /// Represents a handler function that runs when received a "sampling/createMessage" request
    sampling_handler: Option<SamplingHandler>,

    /// Represents a hash map of notification handlers
    notification_handler: Option<Arc<NotificationsHandler>>,
}

impl Roots {
    fn new(options: &McpOptions, notifications_sender: &TransportProtoSender) -> Self {
        let mut roots = Self {
            inner: Arc::new(RwLock::new(options.roots())),
            sender: None
        };

        if options.roots_capability().is_some_and(|roots| roots.list_changed) {
            let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<Root>>(1);
            roots.sender = Some(tx); 
        
            let roots = roots.inner.clone();
            let mut sender = notifications_sender.clone();
            tokio::spawn(async move {
                while let Some(new_roots) = rx.recv().await {
                    let mut current_roots = roots.write().await;
                    *current_roots = new_roots;

                    let changed = Notification::new(
                        crate::types::root::commands::LIST_CHANGED,
                        None);
                    if let Err(_err) = sender.send(changed.into()).await {
                        #[cfg(feature = "tracing")]
                        tracing::error!("Error sending notification: {:?}", _err);
                    }
                }
            });
        }
        
        roots
    }
    
    fn update(&mut self, roots: Vec<Root>) {
        match self.sender.as_mut() {
            None => (),
            Some(sender) => {
                _ = sender
                    .try_send(roots)
                    .map_err(|err| Error::new(ErrorCode::InternalError, err))
            },
        }
    } 
}

impl RequestHandler {
    /// Creates a new [`RequestHandler`]
    pub(super) fn new(transport: TransportProto, options: &McpOptions) -> Self {
        let (tx, rx) = transport.split();
        
        let handler = Self {
            roots: Roots::new(options, &tx),
            counter: AtomicI64::new(1),
            pending: RequestQueue::default(),
            sender: tx,
            timeout: options.timeout,
            sampling_handler: options.sampling_handler.clone(),
            notification_handler: options.notification_handler.clone(),
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
        let id = request.id();
        let receiver = self.pending.push(&id);
        self.sender.send(request.into()).await?;

        match timeout(self.timeout, receiver).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => Err(Error::new(ErrorCode::InternalError, "Response channel closed")),
            Err(_) => {
                _ = self.pending.pop(&id);
                Err(Error::new(ErrorCode::Timeout, "Request timed out"))
            }
        }
    }
    
    /// Sends a notification to MCP server
    #[inline]
    pub(super) async fn send_notification(&mut self, notification: Notification) -> Result<(), Error> {
        self.sender.send(notification.into()).await
    }
    
    /// Updates [`Root`] cache
    pub(super) fn notify_roots_changed(&mut self, roots: Vec<Root>) {
        self.roots.update(roots);
    }

    #[inline]
    #[allow(clippy::single_match)]
    fn start(self, mut rx: TransportProtoReceiver) -> Self {
        let pending = self.pending.clone();
        let mut sender = self.sender.clone();
        let roots = self.roots.inner.clone();
        let sampling_handler = self.sampling_handler.clone();
        let notification_handler = self.notification_handler.clone();
        
        tokio::task::spawn(async move {
            while let Ok(msg) = rx.recv().await {
                match msg {
                    Message::Response(resp) => pending.complete(resp),
                    Message::Request(req) => {
                        match req.method.as_str() { 
                            crate::types::root::commands::LIST => {
                                let roots = {
                                    let roots = roots.read().await;
                                    ListRootsResult::from(roots.to_vec())
                                };
                                sender.send(roots.into_response(req.id()).into()).await.unwrap();    
                            },
                            crate::types::sampling::commands::CREATE => {
                                let id = req.id();
                                let resp = if let Some(handler) = &sampling_handler  {
                                    let result = handler(serde_json::from_value(req.params.unwrap()).unwrap()).await;
                                    result.into_response(id)
                                } else {
                                    Response::error(
                                        id, 
                                        Error::new(ErrorCode::MethodNotFound, "Client does not support sampling requests"))
                                };
                                sender.send(resp.into()).await.unwrap();
                            },
                            _ => {
                                #[cfg(feature = "tracing")]
                                tracing::debug!("Received notification method: {:?}", req.method);
                            }
                        }
                    },
                    Message::Notification(notification) => {
                        match &notification_handler { 
                            Some(handler) => handler.notify(notification).await,
                            None => {
                                #[cfg(feature = "tracing")]
                                notification.write();
                            }
                        }
                    }
                }
            }
        });
        self
    }
}

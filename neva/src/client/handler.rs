//! Request handling utilities

use std::sync::Arc;
use tokio::{sync::RwLock, time::timeout};
use std::{time::Duration, sync::atomic::{AtomicI64, Ordering}};
use crate::client::notification_handler::NotificationsHandler;
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
        sampling::SamplingHandler,
        elicitation::ElicitationHandler
    }
};

#[cfg(feature = "tasks")]
use crate::{
    shared::TaskTracker,
    types::{
        Task, Pagination, CreateTaskResult,
        CreateMessageRequestParams, 
        ElicitRequestParams, 
        ListTasksRequestParams, ListTasksResult,
        CancelTaskRequestParams,
        GetTaskPayloadRequestParams, GetTaskRequestParams
    },
};

#[cfg(feature = "tasks")]
const DEFAULT_PAGE_SIZE: usize = 10;

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

    /// Represents a handler function that runs when received an "elicitation/create" request
    elicitation_handler: Option<ElicitationHandler>,

    /// Represents a hash map of notification handlers
    notification_handler: Option<Arc<NotificationsHandler>>,

    /// Task tracker for client sampling tasks.
    #[cfg(feature = "tasks")]
    tasks: Arc<TaskTracker>
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
            elicitation_handler: options.elicitation_handler.clone(),
            notification_handler: options.notification_handler.clone(),
            #[cfg(feature = "tasks")]
            tasks: Arc::new(TaskTracker::new())
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

    /// Sends the response to MCP server
    #[inline]
    #[cfg(feature = "tasks")]
    pub(super) async fn send_response(&mut self, resp: Response) {
        send_response_impl(&mut self.sender, resp).await;
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
        let elicitation_handler = self.elicitation_handler.clone();
        let notification_handler = self.notification_handler.clone();

        #[cfg(feature = "tasks")]
        let tasks = self.tasks.clone();
        
        tokio::task::spawn(async move {
            while let Ok(msg) = rx.recv().await {
                match msg {
                    Message::Response(resp) => pending.complete(resp),
                    Message::Request(req) => {
                        let resp = match req.method.as_str() { 
                            crate::types::sampling::commands::CREATE => handle_sampling(
                                req, 
                                &sampling_handler, 
                                #[cfg(feature = "tasks")] 
                                &tasks
                            ).await,
                            crate::types::elicitation::commands::CREATE => handle_elicitation(
                                req, 
                                &elicitation_handler, 
                                #[cfg(feature = "tasks")]
                                &tasks
                            ).await,
                            crate::types::root::commands::LIST => handle_roots(req, &roots).await,
                            #[cfg(feature = "tasks")]
                            crate::types::task::commands::RESULT => get_task_result(req, &tasks).await,
                            #[cfg(feature = "tasks")]
                            crate::types::task::commands::LIST => handle_list_tasks(req, &tasks),
                            #[cfg(feature = "tasks")]
                            crate::types::task::commands::CANCEL => cancel_task(req, &tasks),
                            #[cfg(feature = "tasks")]
                            crate::types::task::commands::GET => get_task(req, &tasks),
                            _ => {
                                #[cfg(feature = "tracing")]
                                tracing::debug!("Received notification method: {:?}", req.method);
                                return;
                            }
                        };
                        send_response_impl(&mut sender, resp).await;
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

#[inline]
async fn send_response_impl(sender: &mut TransportProtoSender, resp: Response) {
    if let Err(_err) = sender.send(resp.into()).await {
        #[cfg(feature = "tracing")]
        tracing::error!("Error sending response: {_err:?}");
    }  
}

#[inline]
async fn handle_roots(req: Request, roots: &Arc<RwLock<Vec<Root>>>) -> Response {
    let roots = {
        let roots = roots.read().await;
        ListRootsResult::from(roots.to_vec())
    };
    roots.into_response(req.id())
}

#[inline]
#[cfg(not(feature = "tasks"))]
async fn handle_sampling(req: Request, handler: &Option<SamplingHandler>) -> Response {
    let id = req.id();
    if let Some(handler) = &handler {
        let Some(params) = req.params else {
            return Response::error(id, Error::from(ErrorCode::InvalidParams));
        };
        let Ok(params) = serde_json::from_value(params) else {
            return Response::error(id, Error::from(ErrorCode::ParseError));
        };
        let result = handler(params).await;
        result.into_response(id)
    } else {
        Response::error(
            id, 
            Error::new(
                ErrorCode::MethodNotFound, 
                "Client does not support sampling requests"))
    }
}

#[inline]
#[cfg(feature = "tasks")]
async fn handle_sampling(
    req: Request, 
    handler: &Option<SamplingHandler>,
    tasks: &Arc<TaskTracker>
) -> Response {
    let id = req.id();
    if let Some(handler) = &handler  {
        let Some(params) = req.params else {
            return Response::error(id, Error::from(ErrorCode::InvalidParams));
        };
        let Ok(params) = serde_json::from_value::<CreateMessageRequestParams>(params) else {
            return Response::error(id, Error::from(ErrorCode::ParseError));
        };
        if let Some(task_meta) = params.task {
            let task = Task::from(task_meta);
            let handle = tasks.track(task.clone());

            let task_id = task.id.clone();
            let handler = handler.clone();
            let tasks = tasks.clone();
            tokio::spawn(async move {
                tokio::select! {
                    result = handler(params) => {
                        tasks.complete(&task_id);
                        handle.set_result(result);
                    },
                    _ = handle.cancelled() => {}
                }
            });
            CreateTaskResult::new(task).into_response(id)
        } else {
            let result = handler(params).await;
            result.into_response(id)
        }
    } else {
        Response::error(
            id, 
            Error::new(ErrorCode::MethodNotFound, "Client does not support sampling requests"))
    }
}

#[inline]
#[cfg(not(feature = "tasks"))]
async fn handle_elicitation(req: Request, handler: &Option<ElicitationHandler>) -> Response {
    let id = req.id();
    if let Some(handler) = &handler  {
        let Some(params) = req.params else {
            return Response::error(id, Error::from(ErrorCode::InvalidParams));
        };
        let Ok(params) = serde_json::from_value(params) else {
            return Response::error(id, Error::from(ErrorCode::ParseError));
        };
        let result = handler(params).await;
        result.into_response(id)
    } else {
        Response::error(
            id,
            Error::new(ErrorCode::MethodNotFound, "Client does not support elicitation requests"))
    }
}

#[inline]
#[cfg(feature = "tasks")]
async fn handle_elicitation(
    req: Request, 
    handler: &Option<ElicitationHandler>,
    tasks: &Arc<TaskTracker>
) -> Response {
    let id = req.id();
    if let Some(handler) = &handler  {
        let Some(params) = req.params else {
            return Response::error(id, Error::from(ErrorCode::InvalidParams));
        };
        let Ok(params) = serde_json::from_value(params) else {
            return Response::error(id, Error::from(ErrorCode::ParseError));
        };
        if let ElicitRequestParams::Url(url_params) = &params && 
            let Some(task_meta) = &url_params.task {
            let task = Task::from(*task_meta);
            let handle = tasks.track(task.clone());

            let task_id = task.id.clone();
            let handler = handler.clone();
            let tasks = tasks.clone();
            tokio::spawn(async move {
                tokio::select! {
                    result = handler(params) => {
                        tasks.complete(&task_id);
                        handle.set_result(result);
                    },
                    _ = handle.cancelled() => {}
                }
            });
            CreateTaskResult::new(task).into_response(id)
        } else {
            let result = handler(params).await;
            result.into_response(id)
        }
    } else {
        Response::error(
            id,
            Error::new(ErrorCode::MethodNotFound, "Client does not support elicitation requests"))
    }
}


#[inline]
#[cfg(feature = "tasks")]
fn handle_list_tasks(
    req: Request, 
    tasks: &Arc<TaskTracker>
) -> Response {
    let id = req.id();
    let Some(params) = req.params else {
        return Response::error(id, Error::from(ErrorCode::InvalidParams));
    };
    let params: Option<ListTasksRequestParams> = serde_json::from_value(params).ok();
    ListTasksResult::from(tasks
        .tasks()
        .paginate(
            params.and_then(|p| p.cursor), 
            DEFAULT_PAGE_SIZE))
        .into_response(id)
}

#[inline]
#[cfg(feature = "tasks")]
fn cancel_task(
    req: Request, 
    tasks: &Arc<TaskTracker>
) -> Response {
    let id = req.id();
    let Some(params) = req.params else {
        return Response::error(id, Error::from(ErrorCode::InvalidParams));
    };
    let Ok(params) = serde_json::from_value::<CancelTaskRequestParams>(params) else {
        return Response::error(id, Error::from(ErrorCode::ParseError));
    };
    match tasks.cancel(&params.id) {
        Ok(task) => task.into_response(id),
        Err(err) => Response::error(
            id,
            Error::new(ErrorCode::InvalidParams, err.to_string()))
    }
}

#[inline]
#[cfg(feature = "tasks")]
fn get_task(
    req: Request, 
    tasks: &Arc<TaskTracker>
) -> Response {
    let id = req.id();
    let Some(params) = req.params else {
        return Response::error(id, Error::from(ErrorCode::InvalidParams));
    };
    let Ok(params) = serde_json::from_value::<GetTaskRequestParams>(params) else {
        return Response::error(id, Error::from(ErrorCode::ParseError));
    };
    match tasks.get_status(&params.id) {
        Ok(task) => task.into_response(id),
        Err(err) => Response::error(
            id,
            Error::new(ErrorCode::InvalidParams, err.to_string()))
    }
}

#[inline]
#[cfg(feature = "tasks")]
async fn get_task_result(
    req: Request, 
    tasks: &Arc<TaskTracker>
) -> Response {
    let id = req.id();
    let Some(params) = req.params else {
        return Response::error(id, Error::from(ErrorCode::InvalidParams));
    };
    let Ok(params) = serde_json::from_value::<GetTaskPayloadRequestParams>(params) else {
        return Response::error(id, Error::from(ErrorCode::ParseError));
    };
    match tasks.get_result(&params.id).await {
        Ok(task) => task.into_response(id),
        Err(err) => Response::error(
            id,
            Error::new(ErrorCode::InvalidParams, err.to_string()))
    }
}
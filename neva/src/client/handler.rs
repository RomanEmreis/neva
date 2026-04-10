//! Request handling utilities

use crate::client::notification_handler::NotificationsHandler;
use crate::{
    client::options::McpOptions,
    error::{Error, ErrorCode},
    shared::RequestQueue,
    transport::{
        Receiver, Sender, Transport, TransportProto, TransportProtoReceiver, TransportProtoSender,
    },
    types::{
        IntoResponse, Message, MessageBatch, MessageEnvelope, Request, RequestId, Response, Root,
        elicitation::ElicitationHandler, notification::Notification, root::ListRootsResult,
        sampling::SamplingHandler,
    },
};
use std::sync::Arc;
use std::{
    sync::atomic::{AtomicI64, Ordering},
    time::Duration,
};
use tokio::{sync::RwLock, time::timeout};

#[cfg(feature = "tasks")]
use crate::{
    shared::TaskTracker,
    types::{
        CancelTaskRequestParams, CreateMessageRequestParams, CreateTaskResult, ElicitRequestParams,
        GetTaskPayloadRequestParams, GetTaskRequestParams, ListTasksRequestParams, ListTasksResult,
        Pagination, Task,
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
    tasks: Arc<TaskTracker>,
}

impl Roots {
    fn new(options: &McpOptions, notifications_sender: &TransportProtoSender) -> Self {
        let mut roots = Self {
            inner: Arc::new(RwLock::new(options.roots())),
            sender: None,
        };

        if options
            .roots_capability()
            .is_some_and(|roots| roots.list_changed)
        {
            let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<Root>>(1);
            roots.sender = Some(tx);

            let roots = roots.inner.clone();
            let mut sender = notifications_sender.clone();
            tokio::spawn(async move {
                while let Some(new_roots) = rx.recv().await {
                    let mut current_roots = roots.write().await;
                    *current_roots = new_roots;

                    let changed =
                        Notification::new(crate::types::root::commands::LIST_CHANGED, None);
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
            }
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
            pending: RequestQueue::new(options.timeout),
            sender: tx,
            timeout: options.timeout,
            sampling_handler: options.sampling_handler.clone(),
            elicitation_handler: options.elicitation_handler.clone(),
            notification_handler: options.notification_handler.clone(),
            #[cfg(feature = "tasks")]
            tasks: Arc::new(TaskTracker::new()),
        };

        handler.start(rx)
    }

    /// Returns the next [`RequestId`]
    #[inline]
    pub(super) fn next_id(&self) -> RequestId {
        let id = self.counter.fetch_add(1, Ordering::Relaxed);
        RequestId::Number(id)
    }

    /// Returns the request timeout duration
    #[inline]
    pub(super) fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Returns a reference to the pending request queue
    #[inline]
    pub(super) fn pending(&self) -> &RequestQueue {
        &self.pending
    }

    /// Sends a request to MCP server
    #[inline]
    pub(super) async fn send_request(&mut self, request: Request) -> Result<Response, Error> {
        let id = request.id();
        let receiver = self.pending.push(&id);
        if let Err(err) = self.sender.send(request.into()).await {
            let _ = self.pending.pop(&id);
            return Err(err);
        }
        self.pending.activate(&id);

        match timeout(self.timeout, receiver).await {
            Ok(Ok(resp)) => resp.into_transport_result(),
            Ok(Err(_)) => Err(Error::new(
                ErrorCode::InternalError,
                "Response channel closed",
            )),
            Err(_) => {
                _ = self.pending.pop(&id);
                Err(Error::new(ErrorCode::Timeout, "Request timed out"))
            }
        }
    }

    /// Sends a batch of messages to the MCP server.
    ///
    /// Registers all [`Request`] IDs in the pending queue upfront, sends
    /// `Message::Batch` in a single transport write, and returns a receiver
    /// per request (in input order). [`MessageEnvelope::Notification`] items
    /// are included in the wire payload but produce no receiver slot.
    ///
    /// # Errors
    /// - [`ErrorCode::InvalidRequest`] if `items` is empty (enforced by [`MessageBatch`])
    /// - [`ErrorCode::InvalidRequest`] if `items` contains duplicate request IDs
    /// - Transport error if the underlying sender fails
    pub(super) async fn send_batch(
        &mut self,
        items: Vec<MessageEnvelope>,
    ) -> Result<Vec<(RequestId, tokio::sync::oneshot::Receiver<Response>)>, Error> {
        validate_batch_ids(&items)?;

        let mut receivers = Vec::new();
        let mut envelopes = Vec::new();

        for envelope in items {
            if let MessageEnvelope::Request(ref req) = envelope {
                let id = req.id();
                let receiver = self.pending.push(&id);
                receivers.push((id, receiver));
            }
            envelopes.push(envelope);
        }

        let batch = MessageBatch::new(envelopes)?;
        if let Err(e) = self.sender.send(Message::Batch(batch)).await {
            for (id, _rx) in &receivers {
                let _ = self.pending.pop(id);
            }
            return Err(e);
        }
        for (id, _rx) in &receivers {
            self.pending.activate(id);
        }

        Ok(receivers)
    }

    /// Sends the response to MCP server
    #[inline]
    #[cfg(feature = "tasks")]
    pub(super) async fn send_response(&mut self, resp: Response) {
        send_response_impl(&mut self.sender, resp).await;
    }

    /// Sends a notification to MCP server
    #[inline]
    pub(super) async fn send_notification(
        &mut self,
        notification: Notification,
    ) -> Result<(), Error> {
        self.sender.send(notification.into()).await
    }

    /// Updates [`Root`] cache
    pub(super) fn notify_roots_changed(&mut self, roots: Vec<Root>) {
        self.roots.update(roots);
    }

    #[inline]
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
                        let resp = dispatch_request(
                            req,
                            &roots,
                            &sampling_handler,
                            &elicitation_handler,
                            #[cfg(feature = "tasks")]
                            &tasks,
                        )
                        .await;
                        send_response_impl(&mut sender, resp).await;
                    }
                    Message::Notification(notification) => {
                        dispatch_notification(notification, &notification_handler).await;
                    }
                    Message::Batch(batch) => {
                        // JSON-RPC 2.0 §6 allows either peer to send a batch
                        // containing any mix of Requests, Notifications, and
                        // Responses.
                        //
                        // Drain all Response envelopes first so that waiting
                        // futures aren't gated behind potentially long-running
                        // request handlers (e.g. sampling/elicitation awaiting
                        // user input), which would cause unrelated in-flight
                        // calls to time out even though their responses arrived.
                        let mut deferred = Vec::new();
                        for envelope in batch {
                            match envelope {
                                MessageEnvelope::Response(resp) => pending.complete(resp),
                                other => deferred.push(other),
                            }
                        }
                        // JSON-RPC 2.0 §6: the response to a batch MUST be an
                        // array — collect all per-request responses and send
                        // them back as one Message::Batch rather than as
                        // individual messages.
                        let responses = dispatch_batch_deferred(
                            deferred,
                            &roots,
                            &sampling_handler,
                            &elicitation_handler,
                            &notification_handler,
                            #[cfg(feature = "tasks")]
                            &tasks,
                        )
                        .await;
                        // MessageBatch::new returns Err for an empty vec (all
                        // items were notifications), in which case no reply is
                        // sent — correct per JSON-RPC 2.0 §6.
                        if let Ok(batch) = MessageBatch::new(responses)
                            && let Err(_err) = sender.send(Message::Batch(batch)).await
                        {
                            #[cfg(feature = "tracing")]
                            tracing::error!("Error sending batch response: {_err:?}");
                        }
                    }
                }
            }
        });
        self
    }
}

#[inline]
async fn dispatch_batch_deferred(
    deferred: Vec<MessageEnvelope>,
    roots: &Arc<RwLock<Vec<Root>>>,
    sampling_handler: &Option<SamplingHandler>,
    elicitation_handler: &Option<ElicitationHandler>,
    notification_handler: &Option<Arc<NotificationsHandler>>,
    #[cfg(feature = "tasks")] tasks: &Arc<TaskTracker>,
) -> Vec<MessageEnvelope> {
    use futures_util::future::join_all;

    let futures = deferred.into_iter().map(|envelope| async move {
        match envelope {
            MessageEnvelope::Response(_) => unreachable!(),
            MessageEnvelope::Request(req) => Some(MessageEnvelope::Response(
                dispatch_request(
                    req,
                    roots,
                    sampling_handler,
                    elicitation_handler,
                    #[cfg(feature = "tasks")]
                    tasks,
                )
                .await,
            )),
            MessageEnvelope::Notification(notification) => {
                dispatch_notification(notification, notification_handler).await;
                None
            }
        }
    });

    join_all(futures).await.into_iter().flatten().collect()
}

#[inline]
async fn send_response_impl(sender: &mut TransportProtoSender, resp: Response) {
    if let Err(_err) = sender.send(resp.into()).await {
        #[cfg(feature = "tracing")]
        tracing::error!("Error sending response: {_err:?}");
    }
}

/// Dispatches a server-initiated [`Request`] to the appropriate handler and
/// returns the [`Response`] to send back. Unknown methods produce a
/// [`ErrorCode::MethodNotFound`] error response so the peer is never left
/// waiting for a reply that will never arrive.
#[inline]
async fn dispatch_request(
    req: Request,
    roots: &Arc<RwLock<Vec<Root>>>,
    sampling_handler: &Option<SamplingHandler>,
    elicitation_handler: &Option<ElicitationHandler>,
    #[cfg(feature = "tasks")] tasks: &Arc<TaskTracker>,
) -> Response {
    let req_id = req.id();
    match req.method.as_str() {
        crate::types::sampling::commands::CREATE => {
            handle_sampling(
                req,
                sampling_handler,
                #[cfg(feature = "tasks")]
                tasks,
            )
            .await
        }
        crate::types::elicitation::commands::CREATE => {
            handle_elicitation(
                req,
                elicitation_handler,
                #[cfg(feature = "tasks")]
                tasks,
            )
            .await
        }
        crate::types::root::commands::LIST => handle_roots(req, roots).await,
        #[cfg(feature = "tasks")]
        crate::types::task::commands::RESULT => get_task_result(req, tasks).await,
        #[cfg(feature = "tasks")]
        crate::types::task::commands::LIST => handle_list_tasks(req, tasks),
        #[cfg(feature = "tasks")]
        crate::types::task::commands::CANCEL => cancel_task(req, tasks),
        #[cfg(feature = "tasks")]
        crate::types::task::commands::GET => get_task(req, tasks),
        _ => ErrorCode::MethodNotFound.into_response(req_id),
    }
}

/// Forwards a [`Notification`] to the registered handler or traces it when
/// no handler is configured.
#[inline]
async fn dispatch_notification(
    notification: Notification,
    handler: &Option<Arc<NotificationsHandler>>,
) {
    if let Some(h) = handler {
        h.notify(notification).await
    } else {
        #[cfg(feature = "tracing")]
        notification.write();
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
                "Client does not support sampling requests",
            ),
        )
    }
}

#[inline]
#[cfg(feature = "tasks")]
async fn handle_sampling(
    req: Request,
    handler: &Option<SamplingHandler>,
    tasks: &Arc<TaskTracker>,
) -> Response {
    let id = req.id();
    if let Some(handler) = &handler {
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
            Error::new(
                ErrorCode::MethodNotFound,
                "Client does not support sampling requests",
            ),
        )
    }
}

#[inline]
#[cfg(not(feature = "tasks"))]
async fn handle_elicitation(req: Request, handler: &Option<ElicitationHandler>) -> Response {
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
                "Client does not support elicitation requests",
            ),
        )
    }
}

#[inline]
#[cfg(feature = "tasks")]
async fn handle_elicitation(
    req: Request,
    handler: &Option<ElicitationHandler>,
    tasks: &Arc<TaskTracker>,
) -> Response {
    let id = req.id();
    if let Some(handler) = &handler {
        let Some(params) = req.params else {
            return Response::error(id, Error::from(ErrorCode::InvalidParams));
        };
        let Ok(params) = serde_json::from_value(params) else {
            return Response::error(id, Error::from(ErrorCode::ParseError));
        };
        if let ElicitRequestParams::Url(url_params) = &params
            && let Some(task_meta) = &url_params.task
        {
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
            Error::new(
                ErrorCode::MethodNotFound,
                "Client does not support elicitation requests",
            ),
        )
    }
}

#[inline]
#[cfg(feature = "tasks")]
fn handle_list_tasks(req: Request, tasks: &Arc<TaskTracker>) -> Response {
    let id = req.id();
    let cursor = match req.params {
        None => None,
        Some(p) => match serde_json::from_value::<ListTasksRequestParams>(p) {
            Ok(params) => params.cursor,
            Err(e) => return Response::error(id, Error::new(ErrorCode::InvalidParams, e)),
        },
    };
    ListTasksResult::from(tasks.tasks().paginate(cursor, DEFAULT_PAGE_SIZE)).into_response(id)
}

#[inline]
#[cfg(feature = "tasks")]
fn cancel_task(req: Request, tasks: &Arc<TaskTracker>) -> Response {
    let id = req.id();
    let Some(params) = req.params else {
        return Response::error(id, Error::from(ErrorCode::InvalidParams));
    };
    let Ok(params) = serde_json::from_value::<CancelTaskRequestParams>(params) else {
        return Response::error(id, Error::from(ErrorCode::ParseError));
    };
    match tasks.cancel(&params.id) {
        Ok(task) => task.into_response(id),
        Err(err) => Response::error(id, Error::new(ErrorCode::InvalidParams, err.to_string())),
    }
}

#[inline]
#[cfg(feature = "tasks")]
fn get_task(req: Request, tasks: &Arc<TaskTracker>) -> Response {
    let id = req.id();
    let Some(params) = req.params else {
        return Response::error(id, Error::from(ErrorCode::InvalidParams));
    };
    let Ok(params) = serde_json::from_value::<GetTaskRequestParams>(params) else {
        return Response::error(id, Error::from(ErrorCode::ParseError));
    };
    match tasks.get_status(&params.id) {
        Ok(task) => task.into_response(id),
        Err(err) => Response::error(id, Error::new(ErrorCode::InvalidParams, err.to_string())),
    }
}

#[inline]
#[cfg(feature = "tasks")]
async fn get_task_result(req: Request, tasks: &Arc<TaskTracker>) -> Response {
    let id = req.id();
    let Some(params) = req.params else {
        return Response::error(id, Error::from(ErrorCode::InvalidParams));
    };
    let Ok(params) = serde_json::from_value::<GetTaskPayloadRequestParams>(params) else {
        return Response::error(id, Error::from(ErrorCode::ParseError));
    };
    match tasks.get_result(&params.id).await {
        Ok(task) => task.into_response(id),
        Err(err) => Response::error(id, Error::new(ErrorCode::InvalidParams, err.to_string())),
    }
}

/// Validates that no two [`Request`] envelopes in a batch share the same ID.
///
/// JSON-RPC 2.0 §6 does not explicitly forbid duplicate IDs in a batch, but
/// duplicate IDs make response-to-request correlation ambiguous on the client
/// side — [`crate::shared::RequestQueue::push`] would silently overwrite the
/// earlier waiter, causing it to time out even when a response arrives.
///
/// This is a client-side defensive check, not a spec requirement.
#[inline]
fn validate_batch_ids(items: &[MessageEnvelope]) -> Result<(), Error> {
    let mut seen = std::collections::HashSet::new();
    for envelope in items {
        if let MessageEnvelope::Request(req) = envelope
            && !seen.insert(req.id())
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "batch contains duplicate request IDs",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use tokio::time::Instant;

    #[tokio::test]
    async fn batch_responses_are_distributed_individually() {
        use crate::types::MessageBatch;
        use serde_json::json;
        use tokio::time::{Duration, timeout};

        let queue = RequestQueue::default();

        let id1 = RequestId::Number(1);
        let id2 = RequestId::Number(2);

        let rx1 = queue.push(&id1);
        let rx2 = queue.push(&id2);

        let resp1 = Response::success(id1.clone(), json!({"result": "a"}));
        // A Request envelope in the middle — must be skipped, not completed
        let dummy_req = Request::new(Some(RequestId::Number(99)), "ping", None::<()>);
        let resp2 = Response::success(id2.clone(), json!({"result": "b"}));

        let batch = MessageBatch::new(vec![
            MessageEnvelope::Response(resp1),
            MessageEnvelope::Request(dummy_req),
            MessageEnvelope::Response(resp2),
        ])
        .expect("batch must not be empty");

        // Simulate the batch receive arm
        for envelope in batch {
            if let MessageEnvelope::Response(resp) = envelope {
                queue.complete(resp);
            }
        }

        assert!(
            timeout(Duration::from_millis(100), rx1).await.is_ok(),
            "rx1 should have received its response"
        );
        assert!(
            timeout(Duration::from_millis(100), rx2).await.is_ok(),
            "rx2 should have received its response"
        );
    }

    #[tokio::test]
    async fn batch_requests_are_dispatched_concurrently() {
        use crate::types::sampling::{CreateMessageRequestParams, CreateMessageResult};
        use tokio::time::Duration;

        let roots = Arc::new(RwLock::new(Vec::<Root>::new()));
        let sampling_handler: Option<SamplingHandler> = Some(Arc::new(
            |_params: CreateMessageRequestParams| -> Pin<
                Box<dyn Future<Output = CreateMessageResult> + Send + 'static>,
            > {
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    CreateMessageResult::assistant()
                })
            },
        ));
        let elicitation_handler = None;
        let notification_handler = None;

        let deferred = vec![
            MessageEnvelope::Request(Request::new(
                Some(RequestId::Number(1)),
                crate::types::sampling::commands::CREATE,
                Some(CreateMessageRequestParams::default()),
            )),
            MessageEnvelope::Request(Request::new(
                Some(RequestId::Number(2)),
                crate::types::sampling::commands::CREATE,
                Some(CreateMessageRequestParams::default()),
            )),
        ];

        let started = Instant::now();
        let responses = dispatch_batch_deferred(
            deferred,
            &roots,
            &sampling_handler,
            &elicitation_handler,
            &notification_handler,
            #[cfg(feature = "tasks")]
            &Arc::new(crate::shared::TaskTracker::default()),
        )
        .await;

        assert_eq!(responses.len(), 2);
        assert!(
            started.elapsed() < Duration::from_millis(180),
            "batch requests should run concurrently"
        );
    }

    #[test]
    fn validate_batch_ids_rejects_duplicate_request_ids() {
        let req = |id: i64| {
            MessageEnvelope::Request(Request::new(
                Some(RequestId::Number(id)),
                "ping",
                None::<()>,
            ))
        };

        // Unique IDs — should pass
        assert!(validate_batch_ids(&[req(1), req(2), req(3)]).is_ok());

        // Duplicate ID — should fail
        let err = validate_batch_ids(&[req(1), req(2), req(1)]).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn validate_batch_ids_ignores_notifications() {
        let notif = MessageEnvelope::Notification(crate::types::notification::Notification::new(
            "foo", None,
        ));
        let req =
            MessageEnvelope::Request(Request::new(Some(RequestId::Number(1)), "ping", None::<()>));
        // Two notifications with no ID fields — should not trigger duplicate check
        assert!(validate_batch_ids(&[notif.clone(), req, notif]).is_ok());
    }

    // --- tasks/list omitted-vs-malformed params ---

    #[cfg(feature = "tasks")]
    fn make_tasks_request(params: Option<serde_json::Value>) -> Request {
        Request::new(Some(RequestId::Number(1)), "tasks/list", params)
    }

    #[test]
    #[cfg(feature = "tasks")]
    fn tasks_list_omitted_params_returns_ok() {
        let tasks = Arc::new(crate::shared::TaskTracker::default());
        let req = make_tasks_request(None);
        let resp = handle_list_tasks(req, &tasks);
        assert!(matches!(resp, Response::Ok(_)));
    }

    #[test]
    #[cfg(feature = "tasks")]
    fn tasks_list_empty_object_params_returns_ok() {
        let tasks = Arc::new(crate::shared::TaskTracker::default());
        let req = make_tasks_request(Some(serde_json::json!({})));
        let resp = handle_list_tasks(req, &tasks);
        assert!(matches!(resp, Response::Ok(_)));
    }

    #[test]
    #[cfg(feature = "tasks")]
    fn tasks_list_malformed_cursor_returns_invalid_params() {
        let tasks = Arc::new(crate::shared::TaskTracker::default());
        let req = make_tasks_request(Some(serde_json::json!({"cursor": {"bad": "shape"}})));
        let resp = handle_list_tasks(req, &tasks);
        let Response::Err(err) = resp else {
            panic!("expected error response")
        };
        assert_eq!(err.error.code, ErrorCode::InvalidParams);
    }

    #[test]
    #[cfg(feature = "tasks")]
    fn tasks_list_non_object_params_returns_invalid_params() {
        let tasks = Arc::new(crate::shared::TaskTracker::default());
        let req = make_tasks_request(Some(serde_json::json!("not_an_object")));
        let resp = handle_list_tasks(req, &tasks);
        let Response::Err(err) = resp else {
            panic!("expected error response")
        };
        assert_eq!(err.error.code, ErrorCode::InvalidParams);
    }

    #[test]
    fn send_batch_returns_receiver_per_request_not_notification() {
        // Verifies the queue-registration logic: only Request envelopes get a receiver slot.
        // Full integration is tested via call_batch in client.rs.
        let queue = RequestQueue::default();
        let req_id = RequestId::Number(10);

        // Simulate what send_batch does for a [Notification, Request, Notification] batch
        let notification_1 = MessageEnvelope::Notification(
            crate::types::notification::Notification::new("foo", None),
        );
        let request =
            MessageEnvelope::Request(Request::new(Some(req_id.clone()), "ping", None::<()>));
        let notification_2 = MessageEnvelope::Notification(
            crate::types::notification::Notification::new("bar", None),
        );

        let items = vec![notification_1, request, notification_2];
        let mut receivers = Vec::new();
        for envelope in &items {
            if let MessageEnvelope::Request(req) = envelope {
                let id = req.id();
                let receiver = queue.push(&id);
                receivers.push((id, receiver));
            }
        }

        assert_eq!(
            receivers.len(),
            1,
            "exactly one receiver for the one Request"
        );
        assert_eq!(receivers[0].0, req_id, "receiver ID matches request ID");
    }
}

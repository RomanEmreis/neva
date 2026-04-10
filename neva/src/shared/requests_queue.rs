//! Utilities for tracking requests

use crate::types::{RequestId, Response};
use dashmap::DashMap;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

const DEFAULT_REQUEST_TTL: Duration = Duration::from_secs(10);

/// Represents a request handle
pub(crate) struct RequestHandle {
    sender: oneshot::Sender<Response>,
    _cancellation_token: CancellationToken,
    expires_at: Instant,
}

/// Represents a request tracking "queue" that holds a hash map of [`oneshot::Sender`] for requests
/// that are awaiting responses.
#[derive(Clone)]
pub(crate) struct RequestQueue {
    pending: Arc<DashMap<RequestId, RequestHandle>>,
    ttl: Duration,
}

impl RequestHandle {
    /// Creates a new [`RequestHandle`]
    pub(super) fn new(sender: oneshot::Sender<Response>, ttl: Duration) -> Self {
        Self {
            sender,
            _cancellation_token: CancellationToken::new(),
            expires_at: Instant::now() + ttl,
        }
    }

    /// Sends a [`Response`] to MCP server
    pub(crate) fn send(self, resp: Response) {
        match self.sender.send(resp) {
            Ok(_) => (),
            Err(_err) => {
                #[cfg(feature = "tracing")]
                tracing::error!(
                    logger = "neva",
                    "Request handler failed to send response: {:?}",
                    _err
                );
            }
        };
    }
}

impl RequestQueue {
    /// Creates a new [`RequestQueue`] with the given entry TTL.
    #[inline]
    pub(crate) fn new(ttl: Duration) -> Self {
        Self {
            pending: Arc::new(DashMap::new()),
            ttl,
        }
    }

    /// Pushes a request with [`RequestId`] to the "queue"
    /// and returns a [`oneshot::Receiver`] for the response.
    #[inline]
    pub(crate) fn push(&self, id: &RequestId) -> oneshot::Receiver<Response> {
        self.cleanup_expired();

        let (sender, receiver) = oneshot::channel();
        self.pending
            .insert(id.clone(), RequestHandle::new(sender, self.ttl));
        receiver
    }

    /// Pops the [`RequestHandle`] by [`RequestId`] and removes it from the queue
    #[inline]
    pub(crate) fn pop(&self, id: &RequestId) -> Option<RequestHandle> {
        if self.is_expired(id) {
            let _ = self.pending.remove(id);
            return None;
        }

        self.pending.remove(id).map(|(_, handle)| handle)
    }

    /// Takes a [`Response`] and completes the request if it's still pending
    #[inline]
    pub(crate) fn complete(&self, resp: Response) {
        self.cleanup_expired();

        if let Some(sender) = self.pop(&resp.full_id()) {
            sender.send(resp)
        }
    }

    #[inline]
    fn cleanup_expired(&self) {
        let now = Instant::now();
        self.pending.retain(|_, handle| handle.expires_at > now);
    }

    #[inline]
    fn is_expired(&self, id: &RequestId) -> bool {
        self.pending
            .get(id)
            .is_some_and(|handle| handle.expires_at <= Instant::now())
    }
}

impl Default for RequestQueue {
    #[inline]
    fn default() -> Self {
        Self::new(DEFAULT_REQUEST_TTL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::time::{Duration, timeout};

    #[test]
    fn it_pushes_and_pops_request() {
        let queue = RequestQueue::default();
        let id = RequestId::Number(1);

        let receiver = queue.push(&id);
        let handle = queue.pop(&id);

        assert!(handle.is_some(), "Expected handle to exist");
        assert!(
            queue.pop(&id).is_none(),
            "Handle should be removed after pop"
        );

        drop(receiver); // Avoid warning for unused receiver
    }

    #[tokio::test]
    async fn it_sends_and_receives() {
        let queue = RequestQueue::default();
        let id = RequestId::Number(1);

        let receiver = queue.push(&id);
        let handle = queue.pop(&id).expect("Should have handle");

        let expected = Response::success(id, json!({ "content": "done" }));
        handle.send(expected.clone());
        let Response::Ok(expected) = expected else {
            unreachable!()
        };

        let Response::Ok(actual) = timeout(Duration::from_secs(1), receiver)
            .await
            .expect("Receiver should complete")
            .expect("Sender should send")
        else {
            unreachable!()
        };

        assert_eq!(actual.result, expected.result);
        assert_eq!(actual.id, expected.id);
    }

    #[tokio::test]
    async fn it_sends_response_if_pending() {
        let queue = RequestQueue::default();
        let id = RequestId::Number(1);

        let receiver = queue.push(&id);

        let response = Response::success(id, json!({ "content": "done" }));
        queue.complete(response.clone());

        let Response::Ok(response) = response else {
            unreachable!()
        };

        let Response::Ok(actual) = timeout(Duration::from_secs(1), receiver)
            .await
            .expect("Should receive within timeout")
            .expect("Should receive response")
        else {
            unreachable!()
        };

        assert_eq!(actual.result, response.result);
    }

    #[test]
    fn it_does_nothing_if_not_pending() {
        let queue = RequestQueue::default();
        let id = RequestId::Number(1);

        let response = Response::success(id, json!({ "content": "done" }));

        // No push before complete
        queue.complete(response);

        // Nothing to assert really, just verifying it doesn't panic or error
    }

    #[test]
    fn it_does_remove_expired_pending_requests() {
        let queue = RequestQueue::new(Duration::from_millis(1));
        let id = RequestId::Number(1);

        let _receiver = queue.push(&id);
        std::thread::sleep(Duration::from_millis(10));

        assert!(queue.pop(&id).is_none());
    }

    #[tokio::test]
    async fn pop_does_not_close_non_target_receivers() {
        let queue = RequestQueue::new(Duration::from_millis(5));
        let expired_id = RequestId::Number(1);
        let live_id = RequestId::Number(2);

        let _expired = queue.push(&expired_id);
        let live = queue.push(&live_id);

        std::thread::sleep(Duration::from_millis(10));

        assert!(queue.pop(&expired_id).is_none());

        let response = Response::success(live_id, json!({ "content": "done" }));
        queue.complete(response);

        assert!(
            timeout(Duration::from_secs(1), live).await.is_ok(),
            "non-target receiver should remain open"
        );
    }
}

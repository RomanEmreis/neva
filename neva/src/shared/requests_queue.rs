﻿//! Utilities for tracking requests

use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use crate::types::{RequestId, Response};

/// Represents a request handle
pub(crate) struct RequestHandle {
    sender: oneshot::Sender<Response>,
    _cancellation_token: CancellationToken
}

/// Represents a request tracking "queue" that holds a hash map of [`oneshot::Sender`] for requests
/// that are awaiting responses.
#[derive(Default, Clone)]
pub(crate) struct RequestQueue {
    pending: Arc<DashMap<RequestId, RequestHandle>>
}

impl RequestHandle {
    /// Creates a new [`RequestHandle`]
    pub(super) fn new(sender: oneshot::Sender<Response>) -> Self {
        Self { sender, _cancellation_token: CancellationToken::new() }
    }

    /// Sends a [`Response`] to MCP server
    pub(crate) fn send(self, resp: Response) {
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

impl RequestQueue {
    /// Pushes a request with [`RequestId`] to the "queue" 
    /// and returns a [`oneshot::Receiver`] for the response.
    #[inline]
    pub(crate) fn push(&self, id: &RequestId) -> oneshot::Receiver<Response> {
        let (sender, receiver) = oneshot::channel();
        self.pending.insert(id.clone(), RequestHandle::new(sender));
        receiver
    }

    /// Pops the [`RequestHandle`] by [`RequestId`] and removes it from the queue
    #[inline]
    pub(crate) fn pop(&self, id: &RequestId) -> Option<RequestHandle> {
        self.pending
            .remove(id)
            .map(|(_, handle)| handle)
    }
    
    /// Takes a [`Response`] and completes the request if it's still pending
    #[inline]
    pub(crate) fn complete(&self, resp: Response) {
        if let Some(sender) = self.pop(&resp.full_id()) {
            sender.send(resp)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{timeout, Duration};
    use serde_json::json;

    #[test]
    fn it_pushes_and_pops_request() {
        let queue = RequestQueue::default();
        let id = RequestId::Number(1);

        let receiver = queue.push(&id);
        let handle = queue.pop(&id);

        assert!(handle.is_some(), "Expected handle to exist");
        assert!(queue.pop(&id).is_none(), "Handle should be removed after pop");

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
        let Response::Ok(expected) = expected else { unreachable!() };

        let Response::Ok(actual) = timeout(Duration::from_secs(1), receiver)
            .await
            .expect("Receiver should complete")
            .expect("Sender should send") else { 
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
        
        let Response::Ok(response) = response else { unreachable!() }; 
        
        let Response::Ok(actual) = timeout(Duration::from_secs(1), receiver)
            .await
            .expect("Should receive within timeout")
            .expect("Should receive response") else { 
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
}
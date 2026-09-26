//! Channel pump: drains the App's outbound queue and routes each message
//! either to a pending oneshot (request reply) or to the SSE registry
//! (server-initiated request / notification).

use crate::{shared::SseSessionRegistry, types::Message};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::context::RequestMap;

/// Pumps the App's outbound queue until `token` fires, then empties what is
/// still in it before returning.
///
/// Returning is half of what the transport's drain signal waits for (the
/// engine writing those bytes out is the other half), so the trailing drain
/// below is not an optimisation -- it is what the signal promises.
pub(crate) async fn dispatch(
    pending: RequestMap,
    sse_registry: Arc<SseSessionRegistry>,
    mut sender_rx: mpsc::Receiver<Message>,
    token: CancellationToken,
) {
    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => break,
            Some(msg) = sender_rx.recv() => route(&pending, &sse_registry, msg),
        }
    }

    // Cancellation says stop taking new work, not throw away what is already
    // queued. Everything in the channel at this point was produced by a
    // handler that finished before the teardown reached here -- the
    // graceful-close result of a `subscriptions/listen` stream being the case
    // this exists for. `try_recv` and not `recv`: the senders outlive this
    // task, so awaiting would never end.
    while let Ok(msg) = sender_rx.try_recv() {
        route(&pending, &sse_registry, msg);
    }
}

/// Routes one outbound message: to the oneshot of the request that is waiting
/// for it, or to the SSE registry when nothing is.
///
/// A oneshot that refuses the message belongs to a POST that went away while
/// its handler was still running -- the caller timed out, or a proxy closed
/// the connection. That is the end of that request and nothing more: the
/// response has no one left to go to, and the transport goes on serving
/// everyone else. It used to cancel the transport instead, which let any
/// caller stop the server by giving up on a slow request.
#[inline]
fn route(pending: &RequestMap, sse_registry: &Arc<SseSessionRegistry>, msg: Message) {
    if let Some((_, resp_tx)) = pending.remove(&msg.full_id()) {
        if let Err(_msg) = resp_tx.send(msg) {
            #[cfg(feature = "tracing")]
            tracing::debug!(
                logger = "neva",
                id = %_msg.id(),
                "The caller went away before its response was ready; dropping the response"
            );
        }
    } else if let Err(_e) = sse_registry.send(msg) {
        #[cfg(feature = "tracing")]
        tracing::error!(logger = "neva", "Failed to send server request: {:?}", _e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{RequestId, Response};
    use std::time::Duration;
    use tokio::sync::oneshot;

    /// A POST that went away before its response was ready leaves behind a
    /// oneshot that refuses the response. That ends the one request and
    /// nothing else: the pump drops the response, routes the next one, and
    /// leaves the transport -- whose token this is -- running.
    #[tokio::test]
    async fn an_undeliverable_response_does_not_stop_the_transport() {
        let pending: RequestMap = Arc::new(dashmap::DashMap::new());
        let (gone, gone_rx) = oneshot::channel();
        drop(gone_rx);
        pending.insert(RequestId::Number(1), gone);
        let (waiting, waiting_rx) = oneshot::channel();
        pending.insert(RequestId::Number(2), waiting);

        let (tx, rx) = mpsc::channel(8);
        let token = CancellationToken::new();
        let pump = tokio::spawn(dispatch(
            pending.clone(),
            Arc::new(SseSessionRegistry::new(8)),
            rx,
            token.clone(),
        ));

        for id in [1, 2] {
            tx.send(Message::Response(Response::empty(RequestId::Number(id))))
                .await
                .expect("the pump is running");
        }

        let delivered = tokio::time::timeout(Duration::from_secs(5), waiting_rx)
            .await
            .expect("the next response must still be routed")
            .expect("delivered, not dropped");
        assert_eq!(delivered.id(), RequestId::Number(2));
        assert!(
            !token.is_cancelled(),
            "an undeliverable response must not stop the transport"
        );
        assert!(pending.is_empty());

        token.cancel();
        pump.await.expect("the pump exits once cancelled");
    }
}

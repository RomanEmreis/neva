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
            Some(msg) = sender_rx.recv() => route(&pending, &sse_registry, msg, &token),
        }
    }

    // Cancellation says stop taking new work, not throw away what is already
    // queued. Everything in the channel at this point was produced by a
    // handler that finished before the teardown reached here -- the
    // graceful-close result of a `subscriptions/listen` stream being the case
    // this exists for. `try_recv` and not `recv`: the senders outlive this
    // task, so awaiting would never end.
    while let Ok(msg) = sender_rx.try_recv() {
        route(&pending, &sse_registry, msg, &token);
    }
}

/// Routes one outbound message: to the oneshot of the request that is waiting
/// for it, or to the SSE registry when nothing is.
#[inline]
fn route(
    pending: &RequestMap,
    sse_registry: &Arc<SseSessionRegistry>,
    msg: Message,
    token: &CancellationToken,
) {
    if let Some((_, resp_tx)) = pending.remove(&msg.full_id()) {
        if let Err(_e) = resp_tx.send(msg) {
            #[cfg(feature = "tracing")]
            tracing::error!(logger = "neva", "Failed to send response: {:?}", _e);
            token.cancel();
        }
    } else if let Err(_e) = sse_registry.send(msg) {
        #[cfg(feature = "tracing")]
        tracing::error!(logger = "neva", "Failed to send server request: {:?}", _e);
    }
}

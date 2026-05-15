//! Channel pump: drains the App's outbound queue and routes each message
//! either to a pending oneshot (request reply) or to the SSE registry
//! (server-initiated request / notification).

use crate::{shared::SseSessionRegistry, types::Message};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::context::RequestMap;

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
            Some(msg) = sender_rx.recv() => {
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
        }
    }
}

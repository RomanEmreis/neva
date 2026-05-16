//! Periodic eviction of stale SSE sessions.

use crate::shared::SseSessionRegistry;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

pub(crate) async fn cleanup_stale_sessions(
    sse_registry: Arc<SseSessionRegistry>,
    interval: Duration,
    ttl: Duration,
    token: CancellationToken,
) {
    let mut ticker = tokio::time::interval(interval);

    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => break,
            _ = ticker.tick() => sse_registry.evict_stale(ttl),
        }
    }
}

//! Keeping track of a request's stream so it can be aborted.
//!
//! Over Streamable HTTP under MCP 2026-07-28, closing a request's response
//! stream is how a client cancels it: the server treats the disconnect as the
//! cancellation, and expects no `notifications/cancelled`. A
//! `subscriptions/listen` POST stays open until the subscription is cancelled;
//! any other request is cancelled when the client stops waiting for it, on a
//! timeout or a dropped call. Either way the stream has to be reachable from
//! outside the task driving it, so registration happens in the connection
//! loop, ahead of the spawn, and [`StreamAbort`] carries the handles from
//! there into the request so they are untracked however the request ends.

use super::*;

/// A request's registered abort handle, and its untracking.
///
/// Carried into [`send_request`] rather than made there: registration has to
/// happen in the connection loop, ahead of the spawn -- see [`track_request`].
/// Empty for a message with nothing to abort.
pub(super) struct StreamAbort {
    tokens: Vec<CancellationToken>,
    session: Arc<McpSession>,
    ids: Vec<crate::types::RequestId>,
}

impl StreamAbort {
    /// Whether this message opens a stream that can be aborted at all.
    pub(super) fn is_tracked(&self) -> bool {
        !self.tokens.is_empty()
    }

    /// Resolves as soon as any of the handles is cancelled, or never when there
    /// are none.
    pub(super) async fn cancelled(&self) {
        if self.tokens.is_empty() {
            std::future::pending::<()>().await;
        }

        futures_util::future::select_all(self.tokens.iter().map(|t| Box::pin(t.cancelled()))).await;
    }
}

/// Untracks the handles however [`send_request`] exits -- a transport error, a
/// reply, a cancel, or a panic.
impl Drop for StreamAbort {
    fn drop(&mut self) {
        for id in &self.ids {
            self.session.untrack_stream(id);
        }
    }
}

/// Registers an abort handle for a request's stream.
///
/// Called from the connection loop *before* the request is spawned, not from
/// the spawned task: a `notifications/cancelled` queued right behind a request
/// -- which is what a dropped `Client::listen` sends, and what a call that
/// timed out at once can send -- is read by the very next turn of that loop,
/// and would find nothing to abort if registration were left to the
/// scheduler. Registering here makes the order the order the messages were
/// written in.
///
/// A standalone request qualifies. A batch shares one POST among its requests,
/// so closing it would cancel them all, and a legacy peer cancels by the
/// notification, the close meaning nothing to it; neither is tracked.
pub(super) fn track_request(req: &Message, session: &Arc<McpSession>) -> StreamAbort {
    let ids = match req {
        Message::Request(_) if !session.is_legacy() => request_ids(req),
        _ => Vec::new(),
    };

    let tokens = ids
        .iter()
        .map(|id| session.track_stream(id.clone()))
        .collect();

    StreamAbort {
        tokens,
        session: session.clone(),
        ids,
    }
}

/// Cancels the request a `notifications/cancelled` names the way this
/// transport cancels under MCP 2026-07-28: by closing that request's stream.
///
/// Returns whether the notification is done with here and is not to be sent.
/// It is, for a 2026-07-28 peer: the close is the cancellation, and the spec
/// expects no notification on Streamable HTTP. A legacy peer cancels by the
/// notification itself, so for one it goes out as it is.
pub(super) fn cancel_by_close(msg: &Message, session: &McpSession) -> bool {
    let Message::Notification(notification) = msg else {
        return false;
    };

    if notification.method != crate::types::notification::commands::CANCELLED || session.is_legacy()
    {
        return false;
    }

    if let Some(id) = notification
        .params
        .as_ref()
        .and_then(|p| p.get("requestId"))
        .and_then(|v| <crate::types::RequestId as serde::Deserialize>::deserialize(v).ok())
    {
        session.abort_stream(&id);
    }
    true
}

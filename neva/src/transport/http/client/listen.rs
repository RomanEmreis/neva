//! Keeping track of a `subscriptions/listen` request's stream so it can be
//! aborted.
//!
//! A listen POST stays open indefinitely, which makes it the one request whose
//! stream has to be reachable from outside the task driving it -- a cancelled
//! subscription has to actually close the socket. Registration happens in the
//! connection loop, ahead of the spawn, and [`ListenAbort`] carries the handles
//! from there into the request so they are untracked however the request ends.

use super::*;

/// A request's registered abort handles, and their untracking.
///
/// Carried into [`send_request`] rather than made there: registration has to
/// happen in the connection loop, ahead of the spawn -- see [`track_listen`].
/// Empty for everything that is not a listen.
pub(super) struct ListenAbort {
    tokens: Vec<CancellationToken>,
    session: Arc<McpSession>,
    ids: Vec<crate::types::RequestId>,
}

impl ListenAbort {
    /// Whether this request opens a stream worth aborting at all.
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
/// non-streaming reply, a cancel, or a panic.
impl Drop for ListenAbort {
    fn drop(&mut self) {
        for id in &self.ids {
            self.session.untrack_stream(id);
        }
    }
}

/// Registers abort handles for a request whose reply is a long-lived stream.
///
/// Called from the connection loop *before* the request is spawned, not from
/// the spawned task: a `notifications/cancelled` queued right behind a listen
/// -- which is what a dropped `Client::listen` sends -- is read by the very
/// next turn of that loop, and would find nothing to abort if registration were
/// left to the scheduler. Registering here makes the order the order the
/// messages were written in.
///
/// Only a standalone `subscriptions/listen` qualifies: every other request is
/// answered and gone, so tracking one would be bookkeeping nobody reads, and a
/// batched listen never reaches the transport (`send_batch` rejects it, having
/// no handle to give it a lifetime). Returns nothing tracked for anything else.
pub(super) fn track_listen(req: &Message, session: &Arc<McpSession>) -> ListenAbort {
    let ids = match req {
        Message::Request(r) if r.method == crate::types::subscription::commands::LISTEN => {
            request_ids(req)
        }
        _ => Vec::new(),
    };

    let tokens = ids
        .iter()
        .map(|id| session.track_stream(id.clone()))
        .collect();

    ListenAbort {
        tokens,
        session: session.clone(),
        ids,
    }
}

/// Aborts the streamed reply a `notifications/cancelled` names, if this session
/// is carrying one.
pub(super) fn abort_cancelled_stream(msg: &Message, session: &McpSession) {
    let Message::Notification(notification) = msg else {
        return;
    };

    if notification.method != crate::types::notification::commands::CANCELLED {
        return;
    }

    if let Some(id) = notification
        .params
        .as_ref()
        .and_then(|p| p.get("requestId"))
        .and_then(|v| serde_json::from_value::<crate::types::RequestId>(v.clone()).ok())
    {
        session.abort_stream(&id);
    }
}

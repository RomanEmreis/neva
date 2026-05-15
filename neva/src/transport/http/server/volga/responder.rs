//! Volga-flavored [`SseResponder`] — produces `volga::http::sse::Message`
//! values that the `sse!` macro can stream directly.

use crate::transport::http::core::types::SseResponder;
use crate::types::Message;
use volga::http::sse::Message as SseMessage;

/// SSE responder that emits `volga::http::sse::Message` values.
///
/// Used by the Volga engine inside its GET route. Cheap to clone
/// (unit struct, no state).
///
/// # Example
///
/// ```rust,ignore
/// let stream = handle_get_sse(req, &ctx, &VolgaSseResponder).await;
/// // stream yields volga::http::sse::Message values directly
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct VolgaSseResponder;

impl SseResponder for VolgaSseResponder {
    type Event = SseMessage;

    fn tracked(&self, seq: u64, msg: &Message) -> Self::Event {
        SseMessage::new().id(seq.to_string()).json(msg)
    }

    fn ephemeral(&self, msg: &Message) -> Self::Event {
        SseMessage::new().json(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::notification::Notification;

    fn make_notification() -> Message {
        Message::Notification(Notification::new("test", None))
    }

    #[test]
    fn it_emits_id_field_for_tracked_item() {
        let msg = make_notification();
        let sse = VolgaSseResponder.tracked(42, &msg);
        let debug = format!("{:?}", sse);
        assert!(
            debug.contains("42"),
            "SSE id field must contain the seq number"
        );
    }

    #[test]
    fn it_does_not_emit_id_field_for_ephemeral_item() {
        let msg = make_notification();
        let sse = VolgaSseResponder.ephemeral(&msg);
        let debug = format!("{:?}", sse);
        assert!(
            !debug.contains("id:"),
            "Ephemeral SSE must not have an id field"
        );
    }
}

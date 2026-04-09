//! Tools for binding message channels with MCP Sessions

use crate::error::{Error, ErrorCode};
use crate::types::Message;
use dashmap::DashMap;
use tokio::sync::mpsc::{Sender, error::TrySendError};
use uuid::Uuid;

/// A concurrent message registry that bounds the MCP session ID and related message channel
#[derive(Default)]
pub(crate) struct MessageRegistry {
    inner: DashMap<Uuid, Sender<Message>>,
}

#[allow(dead_code)]
impl MessageRegistry {
    /// Creates a new [`MessageRegistry`]
    #[inline]
    pub(crate) fn new() -> Self {
        Self {
            inner: DashMap::new(),
        }
    }

    /// Registers MCP session channel
    #[inline]
    pub(crate) fn register(&self, key: Uuid, sender: Sender<Message>) {
        self.inner.insert(key, sender);
    }

    /// Unregisters MCP session channel
    #[inline]
    pub(crate) fn unregister(&self, key: &Uuid) -> Option<(Uuid, Sender<Message>)> {
        self.inner.remove(key)
    }

    /// Sends a message into an appropriate channel
    #[inline]
    pub(crate) fn send(&self, message: Message) -> Result<(), Error> {
        let session_id = *message.session_id().ok_or(ErrorCode::InvalidParams)?;

        if let Some(sender) = self.inner.get(&session_id) {
            match sender.try_send(message) {
                Ok(()) => Ok(()),
                Err(err) => {
                    let err_text = err.to_string();
                    match err {
                        TrySendError::Full(_message) => {
                            #[cfg(feature = "tracing")]
                            tracing::warn!(
                                logger = "neva",
                                "Dropping SSE log message for session {}: {}",
                                session_id,
                                err_text
                            );
                            Ok(())
                        }
                        TrySendError::Closed(_message) => {
                            #[cfg(feature = "tracing")]
                            tracing::warn!(
                                logger = "neva",
                                "Failed to deliver SSE log message for session {}: {}",
                                session_id,
                                err_text
                            );
                            Err(Error::new(ErrorCode::InternalError, err_text))
                        }
                    }
                }
            }
        } else {
            Err(Error::new(ErrorCode::InvalidParams, "Sender not found"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Message;
    use crate::types::notification::Notification;
    use tokio::sync::mpsc;

    #[test]
    fn it_creates_new_registry() {
        let registry = MessageRegistry::new();
        assert!(registry.inner.is_empty());
    }

    #[test]
    fn it_registers_and_unregisters() {
        let registry = MessageRegistry::new();
        let session_id = Uuid::new_v4();
        let (tx, _rx) = mpsc::channel(8);

        // Test registration
        registry.register(session_id, tx.clone());
        assert!(registry.inner.contains_key(&session_id));

        // Test unregistration
        let result = registry.unregister(&session_id);
        assert!(result.is_some());
        assert!(!registry.inner.contains_key(&session_id));

        // Test unregistering non-existent session
        let random_id = Uuid::new_v4();
        let result = registry.unregister(&random_id);
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn it_sends_message() {
        let registry = MessageRegistry::new();
        let session_id = Uuid::new_v4();
        let (tx, mut rx) = mpsc::channel(8);

        registry.register(session_id, tx);

        // Create a test message
        let test_message =
            Message::Notification(Notification::new("test", None)).set_session_id(session_id);

        // Send the message
        let send_result = registry.send(test_message);
        assert!(send_result.is_ok());

        // Verify the message received
        let received = rx.recv().await;
        assert!(received.is_some());
        assert_eq!(received.unwrap().session_id(), Some(&session_id));
    }

    #[test]
    fn it_sends_to_nonexistent_session() {
        let registry = MessageRegistry::new();
        let session_id = Uuid::new_v4();

        // Create a test message for a non-existent session
        let test_message =
            Message::Notification(Notification::new("test", None)).set_session_id(session_id);

        // Attempt to send a message
        let send_result = registry.send(test_message);
        assert!(send_result.is_err());
        assert_eq!(send_result.unwrap_err().code, ErrorCode::InvalidParams);
    }

    #[test]
    fn it_sends_message_without_session_id() {
        let registry = MessageRegistry::new();

        // Create a message without session ID
        let test_message = Message::Notification(Notification::new("test", None));

        // Attempt to send a message
        let send_result = registry.send(test_message);
        assert!(send_result.is_err());
        assert_eq!(send_result.unwrap_err().code, ErrorCode::InvalidParams);
    }
}

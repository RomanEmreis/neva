//! Tools and utilities for MCP Client Session

use once_cell::sync::OnceCell;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use crate::transport::http::ServiceUrl;

/// Represents current MCP Session
pub(super) struct McpSession {
    initialized: Notify,
    sse_ready: Notify,
    url: ServiceUrl,
    session_id: OnceCell<uuid::Uuid>,
    cancellation_token: CancellationToken
}

impl McpSession {
    /// Creates a new [`McpSession`]
    pub(super) fn new(url: ServiceUrl, token: CancellationToken) -> Self {
        Self {
            initialized: Notify::new(),
            sse_ready: Notify::new(),
            session_id: OnceCell::new(),
            cancellation_token: token,
            url
        }
    }

    /// Returns a reference to the current MCP Session's [`ServiceUrl`]
    pub(super) fn url(&self) -> &ServiceUrl {
        &self.url
    }

    /// Returns the [`CancellationToken`] that can abort the whole session
    pub(super) fn cancellation_token(&self) -> CancellationToken {
        self.cancellation_token.clone()
    }

    /// Returns `true` if a Session ID has been specified
    pub(super) fn has_session_id(&self) -> bool {
        self.session_id.get().is_some()
    }

    /// Returns a reference to the current MCP Session ID
    pub(super) fn session_id(&self) -> Option<&uuid::Uuid> {
        self.session_id.get()
    }

    /// Sets the MCP Session ID
    pub(super) fn set_session_id(&self, id: uuid::Uuid) {
        if let Err(_err) = self.session_id.set(id) {
            #[cfg(feature = "tracing")]
            tracing::info!("MCP Session Id already set");
        }
    }
    
    /// Sends a signal that this MCP Session has been initialized
    #[inline]
    pub(super) fn notify_session_initialized(&self) {
        self.initialized.notify_one();
    }

    /// Sends a signal that the SSE-connection has been initialized
    #[inline]
    pub(super) fn notify_sse_initialized(&self) {
        self.sse_ready.notify_one();
    }

    /// Waits for MCP Session to be initialized
    #[inline]
    pub(super) async fn initialized(&self) {
        self.initialized.notified().await;
    }

    /// Waits for SSE connection to be initialized
    #[inline]
    pub(super) async fn sse_ready(&self) {
        self.sse_ready.notified().await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use super::*;
    use uuid::Uuid;
    use tokio::time::{timeout, Duration};
    use tokio_util::sync::CancellationToken;
    use crate::transport::http::HttpProto;

    fn create_session() -> McpSession {
        let url = ServiceUrl {
            proto: HttpProto::Http,
            addr: "localhost",
            endpoint: "init",
        };
        let token = CancellationToken::new();
        McpSession::new(url, token)
    }

    #[tokio::test]
    async fn it_has_url() {
        let session = create_session();
        assert_eq!(session.url().addr, "localhost");
        assert_eq!(session.url().endpoint, "init");
    }

    #[tokio::test]
    async fn it_has_cancellable_and_synced_cancellation_token() {
        let session = create_session();
        let token = session.cancellation_token();
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn it_sets_and_gets_session_id() {
        let session = create_session();
        let id = Uuid::new_v4();
        assert!(!session.has_session_id());
        assert!(session.session_id().is_none());

        session.set_session_id(id);
        assert!(session.has_session_id());
        assert_eq!(session.session_id(), Some(&id));
    }

    #[tokio::test]
    async fn it_guarantees_session_id_cannot_be_overwritten() {
        let session = create_session();
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();

        session.set_session_id(id1);
        session.set_session_id(id2); // silently ignored

        assert_eq!(session.session_id(), Some(&id1));
        assert_ne!(session.session_id(), Some(&id2));
    }

    #[tokio::test]
    async fn it_notifies_and_initialized() {
        let session = Arc::new(create_session());

        let handle = tokio::spawn({
            let session = session.clone();
            async move {
                session.initialized().await;
            }
        });

        // Notify after a short_set_and_get delay
        tokio::time::sleep(Duration::from_millis(10)).await;
        session.notify_session_initialized();

        // Should complete within timeout
        assert!(timeout(Duration::from_secs(1), handle).await.is_ok());
    }
}

//! The `subscriptions/listen` handler (MCP 2026-07-28).
//!
//! One long-lived request that stays open for as long as the subscription does.
//! The work is in the ordering: the acknowledgment has to be the first message
//! on the stream, notifications follow while the entry is live, and the request
//! answers with its empty result once the subscription ends -- including when
//! the server is the one ending it.

#[cfg(not(feature = "legacy-spec"))]
use super::*;

/// The `subscriptions/listen` implementation (MCP 2026-07-28).
#[cfg(not(feature = "legacy-spec"))]
impl Context {
    /// Opens a subscription and holds it until it ends.
    ///
    /// The returned future resolves when the client cancels the subscription,
    /// the transport drops it, or the server shuts down; resolving is what
    /// answers the long-lived `subscriptions/listen` request with its
    /// graceful-close result.
    ///
    /// On the shutdown path the answer is delivered, not merely attempted: the
    /// signal ends subscriptions one phase ahead of the transport, waits for
    /// the results they produce to reach the outbound channel, and only then
    /// tears the writers down -- the writers drain what is queued before they
    /// exit, and [`App::run`](crate::App::run) waits for that drain rather
    /// than returning into it, so a host that drops its runtime the moment
    /// `run` returns cannot cut the write short.
    /// [`App::with_shutdown_drain`](crate::App::with_shutdown_drain) caps both
    /// waits together; a server whose subscriptions cannot flush inside that
    /// budget still closes abruptly, which is what the spec tells a client to
    /// treat as a reason to reconnect.
    pub(crate) async fn listen(
        &self,
        id: RequestId,
        requested: SubscriptionFilter,
    ) -> Result<SubscriptionsListenResult, Error> {
        let accepted = requested.supported_by(&self.options.advertised_capabilities());
        let (sink, ack_slot, pump) = self.subscription_sink().await?;

        // The acknowledgment MUST be the first message on the subscription.
        // `register` is what queues it, together with publishing the entry:
        // the two have to be atomic against a concurrent broadcast, and the
        // registry is the only thing that can make them so -- see its docs for
        // why doing it here instead cannot work, whatever the caller does.
        //
        // The sink is empty at this point -- a listen `POST` body carries no
        // request-scoped logging (see `notification::sink::RequestSink`) and a
        // subscription reports no progress -- so `Full` here means a sink far
        // too small to carry a subscription at all, and says so.
        let ack = Notification::new(
            crate::types::subscription::commands::ACKNOWLEDGED,
            serde_json::to_value(SubscriptionsAcknowledgedNotificationParams::new(
                id.clone(),
                accepted.clone(),
            ))
            .ok(),
        );

        let (token, guard) = self.options.subscriptions().register(
            id.clone(),
            self.session_id,
            accepted,
            sink.clone(),
            Message::Notification(ack),
            ack_slot,
        );

        // Whichever comes first: the client cancelled this subscription, the
        // stream went away under us (an HTTP client that closed the response
        // body), or the server is shutting down. The shutdown token is the
        // subscriptions' own, cancelled a phase before the transport's, which
        // is what leaves room for the result below to be written.
        let shutdown = self.options.shutdown_token();
        tokio::select! {
            _ = token.cancelled() => {},
            _ = sink.closed() => {},
            _ = shutdown.cancelled() => {},
        }

        // Deregistering and dropping this end closes the subscription's own
        // channel, which is what lets the pump drain whatever is still queued
        // and exit -- aborting it instead would drop those notifications on the
        // floor a moment before the request answers.
        drop(guard);
        drop(sink);
        if let Some(pump) = pump {
            let _ = pump.await;
        }

        Ok(SubscriptionsListenResult::new(id))
    }

    /// Picks where this subscription's notifications go.
    ///
    /// Over HTTP that is the `POST` response sink the transport registered for
    /// this request -- writing into it puts notifications straight onto the
    /// long-lived response body, and its closure is how the handler learns the
    /// client disconnected. Every other transport (stdio) gets a channel of its
    /// own, pumped into the transport sender by the returned task.
    ///
    /// Comes with the capacity slot reserved for the acknowledgment: over HTTP
    /// the transport took it when it registered the sink, before any middleware
    /// could log into the body, so a noisy request cannot leave the handshake
    /// without room.
    ///
    /// # Errors
    /// Returns [`Error`] for a request that came in over HTTP without a
    /// response stream to write to. A transport session id and no registered
    /// sink means either an engine adapter that cannot stream (the JSON-only
    /// `handlers::handle_post`) or a client that dropped the body before the
    /// runtime got here -- and neither can carry a subscription. Falling back
    /// to a channel of our own would be worse than failing: the acknowledgment
    /// would go to the generic transport sender instead of this request's body,
    /// and since we would then hold the only receiver, the handler would never
    /// see the disconnect that ends it -- the entry would sit in the registry
    /// until the server shut down. A registered sink whose reserved slot is
    /// already gone fails the same way: two listens sharing one body cannot
    /// both open it.
    async fn subscription_sink(
        &self,
    ) -> Result<
        (
            tokio::sync::mpsc::Sender<Message>,
            tokio::sync::mpsc::OwnedPermit<Message>,
            Option<tokio::task::JoinHandle<()>>,
        ),
        Error,
    > {
        #[cfg(feature = "http-server")]
        if let Some(session_id) = self.session_id {
            let stream = crate::types::notification::sink::get(&session_id).zip(
                crate::types::notification::sink::take_ack_permit(&session_id),
            );

            return stream.map(|(sink, ack)| (sink, ack, None)).ok_or_else(|| {
                Error::new(
                    ErrorCode::InternalError,
                    "Subscriptions need a streaming response; this request has none",
                )
            });
        }

        let (tx, mut rx) = tokio::sync::mpsc::channel::<Message>(
            crate::app::subscriptions::DEFAULT_SUBSCRIPTION_CAPACITY,
        );
        // Immediate: nothing has written to this channel yet.
        let ack =
            tx.clone().reserve_owned().await.map_err(|_| {
                Error::new(ErrorCode::InternalError, "Subscription stream is closed")
            })?;

        let mut sender = self.sender.clone();
        let pump = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if sender.send(msg).await.is_err() {
                    break;
                }
            }
        });

        Ok((tx, ack, Some(pump)))
    }
}

#[cfg(all(test, not(feature = "legacy-spec"), feature = "http-server"))]
mod subscription_sink_tests {
    use super::*;

    fn ctx(session_id: Option<uuid::Uuid>) -> Context {
        Context {
            session_id,
            headers: HeaderMap::new(),
            claims: None,
            pending: RequestQueue::new(Duration::from_secs(5)),
            sender: TransportProtoSender::None,
            options: Arc::new(McpOptions::default()),
            timeout: Duration::from_secs(5),
            exec: ExecMode::None,
            client_capabilities: Default::default(),
            #[cfg(feature = "di")]
            scope: None,
        }
    }

    /// A transport session id and no registered sink means the request came in
    /// over HTTP without a response stream -- a JSON-only engine adapter, or a
    /// client that dropped the body first. Falling back to a channel of our own
    /// would send the acknowledgment to the generic transport sender instead of
    /// this request's body, and leave the handler holding the only receiver, so
    /// the disconnect that ends the subscription would never arrive.
    #[tokio::test]
    async fn it_refuses_an_http_request_with_no_response_stream() {
        let err = ctx(Some(uuid::Uuid::new_v4()))
            .subscription_sink()
            .await
            .expect_err("a subscription needs a stream to write to");

        assert_eq!(err.code, ErrorCode::InternalError);
    }

    /// The registered sink is used as is: it *is* the response body, so there
    /// is nothing to pump.
    #[tokio::test]
    async fn it_uses_the_registered_response_sink() {
        let session_id = uuid::Uuid::new_v4();
        let _rx = crate::types::notification::sink::register(session_id, 4, true).await;

        let (_sink, _ack, pump) = ctx(Some(session_id))
            .subscription_sink()
            .await
            .expect("the registered sink must be used");

        assert!(pump.is_none(), "the response body needs no pump task");
        crate::types::notification::sink::unregister(&session_id);
    }

    /// Without a transport session there is no per-request body to write to --
    /// that is stdio, where the subscription interleaves on the shared output
    /// through a pump of its own.
    #[tokio::test]
    async fn it_pumps_into_the_transport_without_a_session() {
        let (_sink, _ack, pump) = ctx(None)
            .subscription_sink()
            .await
            .expect("stdio always has somewhere to write");

        assert!(pump.is_some(), "stdio needs a pump task");
    }
}

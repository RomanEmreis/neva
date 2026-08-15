//! `subscriptions/listen`, and the resource subscriptions that ride on it.
//!
//! Under MCP 2026-07-28 a subscription is one long-lived request whose response
//! stream carries the notifications; [`Client::listen`] opens it and hands back
//! a [`Subscription`](super::Subscription) to hold. The legacy
//! `resources/subscribe` RPCs are still here and still work against a legacy
//! peer -- against a 2026-07-28 one they are refused, since a per-resource
//! subscription is now a filter entry inside a listen stream.

use super::*;

impl Client {
    /// Opens a long-lived notification subscription (MCP 2026-07-28).
    ///
    /// Sends `subscriptions/listen` and returns once the server has
    /// acknowledged the filter. Notifications delivered on the stream are
    /// dispatched to the handlers registered with [`Self::subscribe`] and its
    /// helpers ([`Self::on_tools_changed`] and friends), so those must be in
    /// place before listening -- and, for the capability-gated helpers, after
    /// [`Self::connect`], which is what discovers the capabilities they assert
    /// on.
    ///
    /// The returned [`Subscription`] carries the accepted filter -- the server
    /// silently drops types it does not advertise -- and ends the stream on
    /// [`Subscription::cancel`].
    ///
    /// # Example
    /// ```no_run
    /// use neva::{Client, error::Error, types::SubscriptionFilter};
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///     client.connect().await?;
    ///     client.on_tools_changed(|_| async { println!("tools changed"); });
    ///
    ///     let subscription = client
    ///         .listen(SubscriptionFilter::new().with_tools_changed())
    ///         .await?;
    ///
    ///     println!("accepted: {:?}", subscription.acknowledged());
    ///     Ok(())
    /// }
    /// ```
    #[cfg(not(feature = "legacy-spec"))]
    pub async fn listen(
        &mut self,
        notifications: SubscriptionFilter,
    ) -> Result<Subscription, Error> {
        if self.is_legacy_peer() {
            return Err(Error::new(
                ErrorCode::MethodNotFound,
                "Peer speaks the legacy protocol; use subscribe_to_resource instead",
            ));
        }

        let id = self.generate_id()?;
        let mut request = Request::new(
            Some(id.clone()),
            crate::types::subscription::commands::LISTEN,
            Some(SubscriptionsListenRequestParams::new(notifications.clone())),
        );

        self.apply_client_meta(&mut request, None, None);

        let handler = self
            .handler
            .as_mut()
            .ok_or_else(|| Error::new(ErrorCode::InternalError, "Connection closed"))?;

        // Watch for the acknowledgment before sending: it is the first thing
        // the server puts on the stream, and the receive loop drops one nobody
        // is waiting for.
        let ack = handler.watch_ack(&id, &notifications);
        let sender = handler.sender();
        let release = handler.subscription_release();

        // Armed before the send, not after it. `watch_ack` has already
        // registered the waiter and the pending state, `send_listen` takes the
        // untimed request slot before it awaits the transport, and that await
        // is a suspension point like any other: a caller who drops this future
        // (an outer `tokio::time::timeout`, a lost `select!` branch) runs none
        // of the branches below, and everything registered so far would be left
        // behind. Nothing between here and `watch_ack` awaits, so there is no
        // gap left to fall into.
        let guard =
            subscription::EstablishmentGuard::new(id.clone(), release.clone(), sender.clone());

        let mut response = match handler.send_listen(request).await {
            Ok(response) => response,
            // Never reached the wire, so there is no stream to cancel -- only
            // this client's own bookkeeping to drop.
            Err(err) => {
                guard.forget();
                return Err(err);
            }
        };

        // Race the acknowledgment against the request's own reply: a peer that
        // rejects the subscription outright -- `MethodNotFound`, an
        // authorization failure, invalid params -- answers instead of
        // acknowledging, and waiting only on the acknowledgment would sit out
        // the whole timeout and report that instead of the server's reason.
        let timeout = self.options.timeout;
        let established = tokio::select! {
            biased;
            acknowledged = ack => Ok(acknowledged),
            answered = &mut response => Err(match answered {
                Ok(shared::PendingResponse::Response(resp)) => match resp {
                    // An error reply is the server's own explanation; surface it.
                    Response::Err(err) => err.error.into(),
                    // A success reply this early is the graceful-close result
                    // for a subscription that never carried anything.
                    Response::Ok(_) => Error::new(
                        ErrorCode::InternalError,
                        "Subscription ended before it was acknowledged",
                    ),
                },
                Ok(shared::PendingResponse::Timeout) => {
                    Error::new(ErrorCode::Timeout, "Subscription was not acknowledged")
                }
                // The slot's sender was dropped: the receive loop released it
                // on its way out, so the transport is gone. That is a lost
                // connection, not a peer that would not acknowledge, and
                // callers act on the two differently.
                Err(_) => Error::new(ErrorCode::InternalError, "Connection closed"),
            }),
            _ = tokio::time::sleep(timeout) => Err(Error::new(
                ErrorCode::Timeout,
                "Subscription was not acknowledged",
            )),
        };

        let acknowledged = match established {
            Ok(Ok(filter)) => filter,
            // The waiter's sender was dropped: the receive loop is gone.
            Ok(Err(_)) => {
                guard.abandon().await;
                return Err(Error::new(ErrorCode::InternalError, "Connection closed"));
            }
            Err(err) => {
                guard.abandon().await;
                return Err(err);
            }
        };

        // The server may narrow the filter -- that is the whole point of the
        // acknowledgment -- but it must not widen it. Notifications are
        // dispatched to the client's own handlers with no per-subscription
        // filtering, so an acknowledgment claiming a category or URI this call
        // never asked for would deliver events outside the requested scope.
        if !acknowledged.is_subset_of(&notifications) {
            guard.abandon().await;
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "Server acknowledged a subscription broader than the one requested",
            ));
        }

        // The handle takes over from here.
        guard.disarm();

        Ok(Subscription::new(
            id,
            notifications,
            acknowledged,
            response,
            sender,
            release,
        ))
    }

    /// Subscribes to a resource on the server to receive notifications when it changes.
    ///
    /// Legacy only in effect: MCP 2026-07-28 folds per-resource subscriptions
    /// into the `listen` filter, so against a 2026-07-28 peer this fails and
    /// `listen` with a `resourceSubscriptions` entry is the way. The method
    /// stays available because the dual-mode fallback still reaches legacy
    /// peers.
    pub async fn subscribe_to_resource(&mut self, uri: impl Into<Uri>) -> Result<(), Error> {
        #[cfg(not(feature = "legacy-spec"))]
        if !self.is_legacy_peer() {
            return Err(Error::new(
                ErrorCode::MethodNotFound,
                "resources/subscribe is legacy-only; use listen with a resource filter",
            ));
        }
        if !self.is_resource_subscription_supported() {
            return Err(Error::new(
                ErrorCode::MethodNotFound,
                "Server does not support resource subscriptions",
            ));
        }

        let params = SubscribeRequestParams::from(uri);
        let resp = self
            .command(crate::types::resource::commands::SUBSCRIBE, Some(params))
            .await?;

        match resp {
            Response::Ok(_) => Ok(()),
            Response::Err(err) => Err(err.error.into()),
        }
    }

    /// Unsubscribes from a resource on the server to stop receiving notifications about its changes.
    ///
    /// Legacy only in effect; see [`Self::subscribe_to_resource`]. Under MCP
    /// 2026-07-28 a subscription ends with the stream that carries it
    /// (`Subscription::cancel`).
    pub async fn unsubscribe_from_resource(&mut self, uri: impl Into<Uri>) -> Result<(), Error> {
        #[cfg(not(feature = "legacy-spec"))]
        if !self.is_legacy_peer() {
            return Err(Error::new(
                ErrorCode::MethodNotFound,
                "resources/unsubscribe is legacy-only; cancel the subscription instead",
            ));
        }

        if !self.is_resource_subscription_supported() {
            return Err(Error::new(
                ErrorCode::MethodNotFound,
                "Server does not support resource subscriptions",
            ));
        }

        let params = UnsubscribeRequestParams::from(uri);
        let resp = self
            .command(crate::types::resource::commands::UNSUBSCRIBE, Some(params))
            .await?;

        match resp {
            Response::Ok(_) => Ok(()),
            Response::Err(err) => Err(err.error.into()),
        }
    }
}

/// Establishing a subscription against a peer that misbehaves: answering
/// instead of acknowledging, or acknowledging more than was asked for. Driven
/// by a raw-HTTP mock, since a real neva server produces neither.
#[cfg(all(test, feature = "http-client", not(feature = "legacy-spec")))]
mod listen_rejection_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    const REASON: &str = "subscriptions are disabled here";

    async fn read_request(stream: &mut TcpStream) -> Option<(String, String)> {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 2048];
        let header_end = loop {
            let n = stream.read(&mut tmp).await.ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&tmp[..n]);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
            if buf.len() > 65536 {
                return None;
            }
        };
        let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
        let content_length = head
            .lines()
            .find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|v| v.trim().parse::<usize>().ok())
            })
            .flatten()
            .unwrap_or(0);
        while buf.len() < header_end + content_length {
            let n = stream.read(&mut tmp).await.ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        let body =
            String::from_utf8_lossy(&buf[header_end..header_end + content_length]).to_string();
        Some((head, body))
    }

    async fn write_json(stream: &mut TcpStream, body: &str) {
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(resp.as_bytes()).await;
    }

    /// Answers `server/discover` normally, then rejects `subscriptions/listen`
    /// with a JSON-RPC error and never sends an acknowledgment.
    async fn serve_rejecting(listener: TcpListener) {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                loop {
                    let Some((_head, body)) = read_request(&mut stream).await else {
                        return;
                    };
                    let msg: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                    let id = msg.get("id").cloned().unwrap_or(serde_json::Value::Null);
                    let method = msg
                        .get("method")
                        .and_then(|m| m.as_str())
                        .unwrap_or_default();

                    let reply = if method == crate::commands::DISCOVER {
                        serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {
                                "supportedVersions": [crate::LATEST_PROTOCOL_VERSION],
                                "capabilities": { "tools": { "listChanged": true } }
                            }
                        })
                    } else {
                        serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "error": { "code": -32600, "message": REASON }
                        })
                    };
                    write_json(&mut stream, &reply.to_string()).await;
                }
            });
        }
    }

    /// A cancelled subscription is never answered, so nothing completes its
    /// request slot -- and those slots carry no TTL, because a subscription may
    /// legitimately stay open for hours. Whoever gives up on one has to release
    /// it, or a client that opens and cancels subscriptions in a loop grows the
    /// pending queue by one entry per cycle.
    #[tokio::test(flavor = "multi_thread")]
    async fn cancelling_releases_the_request_slot() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_offside_notification(listener));

        let mut client = Client::new().with_options(|opt| {
            opt.with_http(|http| http.bind(addr.to_string()))
                .with_timeout(std::time::Duration::from_secs(5))
        });
        client.connect().await.expect("connect");

        let queued = |client: &Client| client.handler.as_ref().expect("connected").pending().len();

        let idle = queued(&client);

        let mut subscription = client
            .listen(crate::types::SubscriptionFilter::new().with_tools_changed())
            .await
            .expect("listen");
        assert_eq!(
            queued(&client),
            idle + 1,
            "the live subscription holds one slot"
        );

        subscription.cancel().await.expect("cancel");
        assert_eq!(
            queued(&client),
            idle,
            "cancelling must release the subscription's slot"
        );
    }

    /// A cancel written immediately behind a listen -- which is exactly what a
    /// dropped establishment writes -- has to find the stream it names. The
    /// abort handle is registered by the connection loop rather than by the
    /// task it spawns, so the two are ordered by the wire, not by the
    /// scheduler.
    ///
    /// A single worker on purpose: it pins the interleaving this is about.
    /// Both messages are queued before the connection loop runs, so it reads
    /// the cancel on the turn right after spawning the listen, while that task
    /// has had no chance to run. (One worker rather than `current_thread`
    /// because connecting uses `block_in_place`, which the current-thread
    /// runtime refuses.)
    #[tokio::test(flavor = "multi_thread")]
    async fn a_cancel_queued_behind_a_listen_still_closes_the_stream() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let opened = Arc::new(AtomicBool::new(false));
        let hung_up = Arc::new(AtomicBool::new(false));
        tokio::spawn(serve_orphan_watch(
            listener,
            opened.clone(),
            hung_up.clone(),
        ));

        let mut client = Client::new().with_options(|opt| {
            opt.with_http(|http| http.bind(addr.to_string()))
                .with_timeout(std::time::Duration::from_secs(30))
        });
        client.connect().await.expect("connect");

        use crate::transport::Sender as _;

        let id = RequestId::Number(99);
        let mut sender = client.handler.as_ref().expect("connected").sender();

        let listen = Request::new(
            Some(id.clone()),
            crate::types::subscription::commands::LISTEN,
            Some(SubscriptionsListenRequestParams::new(
                crate::types::SubscriptionFilter::new().with_tools_changed(),
            )),
        );
        sender.send(listen.into()).await.expect("send listen");
        sender
            .send(subscription::cancelled(&id).into())
            .await
            .expect("send cancel");

        // The abort may land before the POST is even written, so what is
        // asserted is the outcome rather than one particular mechanism: the
        // peer must not be left holding this listen open. Without the fix the
        // cancel finds no handle, the request goes out, and the stream drains
        // for as long as the connection lives.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        while !hung_up.load(AtomicOrdering::SeqCst) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }

        assert!(
            !opened.load(AtomicOrdering::SeqCst) || hung_up.load(AtomicOrdering::SeqCst),
            "a cancel arriving right behind its listen left the peer holding an orphaned stream"
        );
    }

    /// Answers `server/discover`, then holds any other request open and reports
    /// both that it saw one and whether the client ever hung up on it.
    async fn serve_orphan_watch(
        listener: TcpListener,
        opened: Arc<AtomicBool>,
        hung_up: Arc<AtomicBool>,
    ) {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let (opened, hung_up) = (opened.clone(), hung_up.clone());
            tokio::spawn(async move {
                loop {
                    let Some((_head, body)) = read_request(&mut stream).await else {
                        return;
                    };

                    let msg: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                    let id = msg.get("id").cloned().unwrap_or(serde_json::Value::Null);
                    let method = msg
                        .get("method")
                        .and_then(|m| m.as_str())
                        .unwrap_or_default();

                    if method == crate::commands::DISCOVER {
                        let reply = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {
                                "supportedVersions": [crate::LATEST_PROTOCOL_VERSION],
                                "capabilities": { "tools": { "listChanged": true } }
                            }
                        });
                        write_json(&mut stream, &reply.to_string()).await;
                        continue;
                    }

                    // Anything else -- the cancel notification travels on its
                    // own `POST` -- is answered and forgotten; only the listen
                    // is the stream this test is about.
                    if method != crate::types::subscription::commands::LISTEN {
                        let _ = stream
                            .write_all(
                                b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: keep-alive\r\n\r\n",
                            )
                            .await;
                        continue;
                    }

                    opened.store(true, AtomicOrdering::SeqCst);

                    let mut probe = [0u8; 1];
                    if let Ok(Ok(0)) = tokio::time::timeout(
                        std::time::Duration::from_secs(3),
                        stream.read(&mut probe),
                    )
                    .await
                    {
                        hung_up.store(true, AtomicOrdering::SeqCst);
                    }

                    return;
                }
            });
        }
    }

    /// A caller who drops the `listen` future -- an outer `timeout`, a lost
    /// `select!` branch -- runs none of the error paths inside it, so the
    /// bookkeeping and the peer's stream would be left behind by an
    /// establishment that never returned anything to end them with.
    #[tokio::test(flavor = "multi_thread")]
    async fn dropping_the_listen_future_abandons_the_subscription() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let closed = Arc::new(AtomicBool::new(false));
        tokio::spawn(serve_stalled_listen(listener, closed.clone()));

        let mut client = Client::new().with_options(|opt| {
            opt.with_http(|http| http.bind(addr.to_string()))
                // Far longer than this test waits: the establishment must be
                // ended by the dropped future, not by `listen`'s own timeout.
                .with_timeout(std::time::Duration::from_secs(30))
        });
        client.connect().await.expect("connect");

        let queued = |client: &Client| client.handler.as_ref().expect("connected").pending().len();
        let idle = queued(&client);

        // The caller gives up on its own schedule and never sees a result.
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(300),
                client.listen(crate::types::SubscriptionFilter::new().with_tools_changed()),
            )
            .await
            .is_err(),
            "the peer never acknowledges, so the outer timeout must fire"
        );

        assert_eq!(
            queued(&client),
            idle,
            "a dropped establishment must release its request slot"
        );

        // And it has to reach the wire too: the peer is still holding the
        // listen request open, waiting for someone who is no longer there.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while !closed.load(AtomicOrdering::SeqCst) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(
            closed.load(AtomicOrdering::SeqCst),
            "a dropped establishment must close the stream it opened"
        );
    }

    /// A cancel that never reached the wire ended nothing -- the connection
    /// did. Reporting `Cancelled` for it would tell the caller a deliberate
    /// stop happened where a connection loss did, and reconnect logic keys on
    /// exactly that difference.
    #[tokio::test(flavor = "multi_thread")]
    async fn cancelling_after_a_disconnect_reports_abrupt() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_offside_notification(listener));

        let mut client = Client::new().with_options(|opt| {
            opt.with_http(|http| http.bind(addr.to_string()))
                .with_timeout(std::time::Duration::from_secs(5))
        });
        client.connect().await.expect("connect");

        let mut subscription = client
            .listen(crate::types::SubscriptionFilter::new().with_tools_changed())
            .await
            .expect("listen");

        client.disconnect().await.expect("disconnect");

        assert!(
            subscription.cancel().await.is_err(),
            "a cancel has nowhere to go once the transport is gone"
        );

        let ended = tokio::time::timeout(std::time::Duration::from_secs(2), subscription.closed())
            .await
            .expect("closed() must not hang after a failed cancel");
        assert!(
            matches!(ended, crate::client::SubscriptionEnd::Abrupt),
            "got {ended:?}"
        );
    }

    /// The same slot must come back when the handle is simply dropped.
    #[tokio::test(flavor = "multi_thread")]
    async fn dropping_the_handle_releases_the_request_slot() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_offside_notification(listener));

        let mut client = Client::new().with_options(|opt| {
            opt.with_http(|http| http.bind(addr.to_string()))
                .with_timeout(std::time::Duration::from_secs(5))
        });
        client.connect().await.expect("connect");

        let queued = |client: &Client| client.handler.as_ref().expect("connected").pending().len();

        let idle = queued(&client);
        {
            let _subscription = client
                .listen(crate::types::SubscriptionFilter::new().with_tools_changed())
                .await
                .expect("listen");
            assert_eq!(queued(&client), idle + 1);
        }

        assert_eq!(
            queued(&client),
            idle,
            "dropping the handle must release the subscription's slot"
        );
    }

    /// Acknowledges exactly what was asked for, then sends a tagged
    /// notification of a category outside it.
    async fn serve_offside_notification(listener: TcpListener) {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                loop {
                    let Some((_head, body)) = read_request(&mut stream).await else {
                        return;
                    };
                    let msg: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                    let id = msg.get("id").cloned().unwrap_or(serde_json::Value::Null);
                    let method = msg
                        .get("method")
                        .and_then(|m| m.as_str())
                        .unwrap_or_default();

                    if method == crate::commands::DISCOVER {
                        let reply = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {
                                "supportedVersions": [crate::LATEST_PROTOCOL_VERSION],
                                "capabilities": {
                                    "tools": { "listChanged": true },
                                    "prompts": { "listChanged": true }
                                }
                            }
                        });
                        write_json(&mut stream, &reply.to_string()).await;
                        continue;
                    }

                    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\n";
                    let _ = stream.write_all(head.as_bytes()).await;

                    // A correct acknowledgment: tools only, exactly as asked.
                    let ack = serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/subscriptions/acknowledged",
                        "params": {
                            "notifications": { "toolsListChanged": true },
                            "_meta": { crate::types::SUBSCRIPTION_ID_KEY: id }
                        }
                    });
                    let _ = stream
                        .write_all(format!("data: {ack}\n\n").as_bytes())
                        .await;

                    // ...then a category the filter never selected.
                    let offside = serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": crate::types::prompt::commands::LIST_CHANGED,
                        "params": { "_meta": { crate::types::SUBSCRIPTION_ID_KEY: id } }
                    });
                    let _ = stream
                        .write_all(format!("data: {offside}\n\n").as_bytes())
                        .await;

                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    return;
                }
            });
        }
    }

    /// A valid acknowledgment is a promise about the whole stream, not just its
    /// first message. A peer that keeps it and then sends an off-filter
    /// notification anyway must not reach the client's handlers -- they are
    /// global and know nothing about which subscription a message came from.
    #[tokio::test(flavor = "multi_thread")]
    async fn notifications_outside_the_acknowledged_filter_are_dropped() {
        use std::sync::atomic::AtomicUsize;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_offside_notification(listener));

        let tools = Arc::new(AtomicUsize::new(0));
        let prompts = Arc::new(AtomicUsize::new(0));
        let (tools_seen, prompts_seen) = (tools.clone(), prompts.clone());

        let mut client = Client::new().with_options(|opt| {
            opt.with_http(|http| http.bind(addr.to_string()))
                .with_timeout(std::time::Duration::from_secs(5))
        });
        client.connect().await.expect("connect");
        client.subscribe(crate::types::tool::commands::LIST_CHANGED, move |_| {
            let seen = tools_seen.clone();
            async move {
                seen.fetch_add(1, AtomicOrdering::SeqCst);
            }
        });
        client.subscribe(crate::types::prompt::commands::LIST_CHANGED, move |_| {
            let seen = prompts_seen.clone();
            async move {
                seen.fetch_add(1, AtomicOrdering::SeqCst);
            }
        });

        let _subscription = client
            .listen(crate::types::SubscriptionFilter::new().with_tools_changed())
            .await
            .expect("listen");

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        assert_eq!(
            prompts.load(AtomicOrdering::SeqCst),
            0,
            "a notification outside the acknowledged filter must be dropped"
        );
        assert_eq!(
            tools.load(AtomicOrdering::SeqCst),
            0,
            "and the in-filter categories are unaffected (none were sent)"
        );
    }

    /// Answers `server/discover` normally, then puts a correctly tagged,
    /// in-filter notification on the stream *ahead* of the acknowledgment.
    async fn serve_notification_before_ack(listener: TcpListener) {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                loop {
                    let Some((_head, body)) = read_request(&mut stream).await else {
                        return;
                    };
                    let msg: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                    let id = msg.get("id").cloned().unwrap_or(serde_json::Value::Null);
                    let method = msg
                        .get("method")
                        .and_then(|m| m.as_str())
                        .unwrap_or_default();

                    if method == crate::commands::DISCOVER {
                        let reply = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {
                                "supportedVersions": [crate::LATEST_PROTOCOL_VERSION],
                                "capabilities": { "tools": { "listChanged": true } }
                            }
                        });
                        write_json(&mut stream, &reply.to_string()).await;
                        continue;
                    }

                    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\n";
                    let _ = stream.write_all(head.as_bytes()).await;

                    // Correctly tagged, squarely inside what was requested --
                    // and out of order.
                    let early = serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": crate::types::tool::commands::LIST_CHANGED,
                        "params": { "_meta": { crate::types::SUBSCRIPTION_ID_KEY: id } }
                    });
                    let _ = stream
                        .write_all(format!("data: {early}\n\n").as_bytes())
                        .await;

                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    return;
                }
            });
        }
    }

    /// The acknowledgment comes first or the subscription is not established.
    /// A peer that streams before acknowledging is streaming from a
    /// subscription `listen` goes on to report as failed -- its events must not
    /// have reached the handlers in the meantime.
    #[tokio::test(flavor = "multi_thread")]
    async fn notifications_before_the_acknowledgment_are_dropped() {
        use std::sync::atomic::AtomicUsize;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_notification_before_ack(listener));

        let tools = Arc::new(AtomicUsize::new(0));
        let seen = tools.clone();

        let mut client = Client::new().with_options(|opt| {
            opt.with_http(|http| http.bind(addr.to_string()))
                // Bounded, because the peer never acknowledges and
                // establishment has to end by timing out -- but not tight: the
                // same budget covers the `connect` above, which shares this
                // client, and a loaded machine makes a short one flake.
                .with_timeout(std::time::Duration::from_secs(5))
        });
        client.connect().await.expect("connect");
        client.subscribe(crate::types::tool::commands::LIST_CHANGED, move |_| {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, AtomicOrdering::SeqCst);
            }
        });

        client
            .listen(crate::types::SubscriptionFilter::new().with_tools_changed())
            .await
            .expect_err("a peer that never acknowledges must not establish");

        assert_eq!(
            tools.load(AtomicOrdering::SeqCst),
            0,
            "a notification sent before the acknowledgment must be dropped"
        );
    }

    /// Answers `server/discover` normally, acknowledges the subscription, then
    /// pushes a subscribable notification with no subscription id on it.
    async fn serve_untagged_notification(listener: TcpListener) {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                loop {
                    let Some((_head, body)) = read_request(&mut stream).await else {
                        return;
                    };
                    let msg: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                    let id = msg.get("id").cloned().unwrap_or(serde_json::Value::Null);
                    let method = msg
                        .get("method")
                        .and_then(|m| m.as_str())
                        .unwrap_or_default();

                    if method == crate::commands::DISCOVER {
                        let reply = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {
                                "supportedVersions": [crate::LATEST_PROTOCOL_VERSION],
                                "capabilities": { "tools": { "listChanged": true } }
                            }
                        });
                        write_json(&mut stream, &reply.to_string()).await;
                        continue;
                    }

                    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\n";
                    let _ = stream.write_all(head.as_bytes()).await;

                    let ack = serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/subscriptions/acknowledged",
                        "params": {
                            "notifications": { "toolsListChanged": true },
                            "_meta": { crate::types::SUBSCRIPTION_ID_KEY: id }
                        }
                    });
                    let _ = stream
                        .write_all(format!("data: {ack}\n\n").as_bytes())
                        .await;

                    // In the accepted filter, but with nothing tying it to the
                    // subscription that accepted it.
                    let untagged = serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": crate::types::tool::commands::LIST_CHANGED
                    });
                    let _ = stream
                        .write_all(format!("data: {untagged}\n\n").as_bytes())
                        .await;

                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    return;
                }
            });
        }
    }

    /// Under MCP 2026-07-28 a subscribable notification travels on a
    /// subscription and nowhere else. One that arrives without a subscription
    /// id has nothing to check it against, so it cannot be admitted just
    /// because the client happens to have asked for that category somewhere.
    #[tokio::test(flavor = "multi_thread")]
    async fn untagged_subscribable_notifications_are_dropped() {
        use std::sync::atomic::AtomicUsize;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_untagged_notification(listener));

        let tools = Arc::new(AtomicUsize::new(0));
        let seen = tools.clone();

        let mut client = Client::new().with_options(|opt| {
            opt.with_http(|http| http.bind(addr.to_string()))
                .with_timeout(std::time::Duration::from_secs(5))
        });
        client.connect().await.expect("connect");
        client.subscribe(crate::types::tool::commands::LIST_CHANGED, move |_| {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, AtomicOrdering::SeqCst);
            }
        });

        let _subscription = client
            .listen(crate::types::SubscriptionFilter::new().with_tools_changed())
            .await
            .expect("listen");

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        assert_eq!(
            tools.load(AtomicOrdering::SeqCst),
            0,
            "a subscription-only notification with no subscription id must be dropped"
        );
    }

    /// A transport that dies while `listen` is still waiting for its
    /// acknowledgment is a lost connection, not a peer that would not
    /// acknowledge -- and callers act on those differently.
    #[tokio::test(flavor = "multi_thread")]
    async fn listen_reports_a_lost_transport_as_a_connection_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_stalled_listen(
            listener,
            Arc::new(AtomicBool::new(false)),
        ));

        let mut client = Client::new().with_options(|opt| {
            opt.with_http(|http| http.bind(addr.to_string()))
                // Long enough that a timeout cannot be what ends the wait.
                .with_timeout(std::time::Duration::from_secs(30))
        });
        client.connect().await.expect("connect");

        let token = client.handler.as_ref().expect("connected").cancellation();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            token.cancel();
        });

        let err = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.listen(crate::types::SubscriptionFilter::new().with_tools_changed()),
        )
        .await
        .expect("a dead transport must not be waited out")
        .expect_err("a dead transport cannot establish a subscription");

        assert_eq!(err.code, ErrorCode::InternalError, "got {err:?}");
    }

    /// Answers `server/discover` normally, then delivers the whole
    /// subscription -- acknowledgment, an in-filter notification and an
    /// off-filter one -- inside a single JSON-RPC batch.
    async fn serve_batched_frames(listener: TcpListener) {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };

            tokio::spawn(async move {
                loop {
                    let Some((_head, body)) = read_request(&mut stream).await else {
                        return;
                    };

                    let msg: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                    let id = msg.get("id").cloned().unwrap_or(serde_json::Value::Null);
                    let method = msg
                        .get("method")
                        .and_then(|m| m.as_str())
                        .unwrap_or_default();

                    if method == crate::commands::DISCOVER {
                        let reply = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {
                                "supportedVersions": [crate::LATEST_PROTOCOL_VERSION],
                                "capabilities": {
                                    "tools": { "listChanged": true },
                                    "prompts": { "listChanged": true }
                                }
                            }
                        });

                        write_json(&mut stream, &reply.to_string()).await;
                        continue;
                    }

                    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\n";
                    let _ = stream.write_all(head.as_bytes()).await;

                    let batch = serde_json::json!([
                        {
                            "jsonrpc": "2.0",
                            "method": "notifications/subscriptions/acknowledged",
                            "params": {
                                "notifications": { "toolsListChanged": true },
                                "_meta": { crate::types::SUBSCRIPTION_ID_KEY: id }
                            }
                        },
                        {
                            "jsonrpc": "2.0",
                            "method": crate::types::tool::commands::LIST_CHANGED,
                            "params": { "_meta": { crate::types::SUBSCRIPTION_ID_KEY: id } }
                        },
                        {
                            "jsonrpc": "2.0",
                            "method": crate::types::prompt::commands::LIST_CHANGED,
                            "params": { "_meta": { crate::types::SUBSCRIPTION_ID_KEY: id } }
                        }
                    ]);
                    let _ = stream
                        .write_all(format!("data: {batch}\n\n").as_bytes())
                        .await;

                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    return;
                }
            });
        }
    }

    /// Batching is a framing choice of the peer's. An acknowledgment sent that
    /// way still has to establish the subscription, and a tagged notification
    /// sent that way still has to face the filter it was accepted under.
    #[tokio::test(flavor = "multi_thread")]
    async fn batched_subscription_frames_go_through_the_same_gate() {
        use std::sync::atomic::AtomicUsize;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_batched_frames(listener));

        let tools = Arc::new(AtomicUsize::new(0));
        let prompts = Arc::new(AtomicUsize::new(0));
        let (tools_seen, prompts_seen) = (tools.clone(), prompts.clone());

        let mut client = Client::new().with_options(|opt| {
            opt.with_http(|http| http.bind(addr.to_string()))
                .with_timeout(std::time::Duration::from_secs(30))
        });
        client.connect().await.expect("connect");
        client.subscribe(crate::types::tool::commands::LIST_CHANGED, move |_| {
            let seen = tools_seen.clone();
            async move {
                seen.fetch_add(1, AtomicOrdering::SeqCst);
            }
        });
        client.subscribe(crate::types::prompt::commands::LIST_CHANGED, move |_| {
            let seen = prompts_seen.clone();
            async move {
                seen.fetch_add(1, AtomicOrdering::SeqCst);
            }
        });

        // A batched acknowledgment has to resolve `listen`, not leave it
        // waiting out the establishment timeout.
        let _subscription = client
            .listen(crate::types::SubscriptionFilter::new().with_tools_changed())
            .await
            .expect("a batched acknowledgment must establish the subscription");

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        assert_eq!(
            tools.load(AtomicOrdering::SeqCst),
            1,
            "an in-filter batched notification must be delivered"
        );
        assert_eq!(
            prompts.load(AtomicOrdering::SeqCst),
            0,
            "an off-filter batched notification must be dropped"
        );
    }

    /// Answers `server/discover` normally, then acknowledges the subscription
    /// with a *broader* filter than the client asked for.
    async fn serve_overbroad_ack(listener: TcpListener) {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                loop {
                    let Some((_head, body)) = read_request(&mut stream).await else {
                        return;
                    };
                    let msg: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                    let id = msg.get("id").cloned().unwrap_or(serde_json::Value::Null);
                    let method = msg
                        .get("method")
                        .and_then(|m| m.as_str())
                        .unwrap_or_default();

                    if method == crate::commands::DISCOVER {
                        let reply = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {
                                "supportedVersions": [crate::LATEST_PROTOCOL_VERSION],
                                "capabilities": {
                                    "tools": { "listChanged": true },
                                    "prompts": { "listChanged": true }
                                }
                            }
                        });
                        write_json(&mut stream, &reply.to_string()).await;
                        continue;
                    }

                    // The client asked for tools only; claim prompts as well.
                    let ack = serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/subscriptions/acknowledged",
                        "params": {
                            "notifications": {
                                "toolsListChanged": true,
                                "promptsListChanged": true
                            },
                            "_meta": { crate::types::SUBSCRIPTION_ID_KEY: id }
                        }
                    });
                    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\n";
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream
                        .write_all(format!("data: {ack}\n\n").as_bytes())
                        .await;

                    // Straight behind it, a notification squarely inside what
                    // the client *did* ask for. The acknowledgment is on its way
                    // to being rejected, so this must not reach the handlers
                    // either -- intersecting the filter alone would let it.
                    let in_filter = serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": crate::types::tool::commands::LIST_CHANGED,
                        "params": { "_meta": { crate::types::SUBSCRIPTION_ID_KEY: id } }
                    });
                    let _ = stream
                        .write_all(format!("data: {in_filter}\n\n").as_bytes())
                        .await;

                    // Hold the stream open, the way a real subscription would.
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    return;
                }
            });
        }
    }

    /// An acknowledgment may narrow the requested filter -- that is what it is
    /// for -- but never widen it. Notifications reach the client's global
    /// handlers with no per-subscription filtering, so accepting an overbroad
    /// acknowledgment would deliver events this call never asked for.
    #[tokio::test(flavor = "multi_thread")]
    async fn listen_rejects_an_overbroad_acknowledgment() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_overbroad_ack(listener));

        let mut client = Client::new().with_options(|opt| {
            opt.with_http(|http| http.bind(addr.to_string()))
                .with_timeout(std::time::Duration::from_secs(5))
        });
        client.connect().await.expect("connect");

        let tools = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = tools.clone();
        client.subscribe(crate::types::tool::commands::LIST_CHANGED, move |_| {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, AtomicOrdering::SeqCst);
            }
        });

        let err = client
            .listen(crate::types::SubscriptionFilter::new().with_tools_changed())
            .await
            .expect_err("an overbroad acknowledgment must be rejected");

        let reported = format!("{err:?}");
        assert!(
            reported.contains("broader"),
            "the rejection must name the cause, got: {reported}"
        );

        // Nothing may have been delivered on the strength of an acknowledgment
        // this call refuses -- not even a category it did ask for.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert_eq!(
            tools.load(AtomicOrdering::SeqCst),
            0,
            "a subscription `listen` rejects must deliver nothing"
        );
    }

    /// Answers `server/discover` normally, then accepts the listen `POST` and
    /// never replies at all -- headers included. Records whether that
    /// connection was closed by the peer.
    async fn serve_stalled_listen(listener: TcpListener, closed: Arc<AtomicBool>) {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let closed = closed.clone();
            tokio::spawn(async move {
                loop {
                    let Some((_head, body)) = read_request(&mut stream).await else {
                        return;
                    };
                    let msg: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                    let id = msg.get("id").cloned().unwrap_or(serde_json::Value::Null);
                    let method = msg
                        .get("method")
                        .and_then(|m| m.as_str())
                        .unwrap_or_default();

                    if method == crate::commands::DISCOVER {
                        let reply = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {
                                "supportedVersions": [crate::LATEST_PROTOCOL_VERSION],
                                "capabilities": { "tools": { "listChanged": true } }
                            }
                        });
                        write_json(&mut stream, &reply.to_string()).await;
                        continue;
                    }

                    // Sit on the request without sending so much as a status
                    // line, and watch for the client hanging up.
                    let mut probe = [0u8; 1];
                    let hung_up = tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        stream.read(&mut probe),
                    )
                    .await
                    .is_ok_and(|read| matches!(read, Ok(0)));
                    if hung_up {
                        closed.store(true, AtomicOrdering::SeqCst);
                    }
                    return;
                }
            });
        }
    }

    /// A subscription can be cancelled while the peer is still sitting on the
    /// response headers -- establishment timing out is exactly that. The abort
    /// handle has to exist by then, or the cancel finds nothing to close and the
    /// task starts draining an orphaned stream once the headers finally arrive.
    #[tokio::test(flavor = "multi_thread")]
    async fn listen_closes_a_stalled_request_on_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let closed = Arc::new(AtomicBool::new(false));
        tokio::spawn(serve_stalled_listen(listener, closed.clone()));

        let mut client = Client::new().with_options(|opt| {
            opt.with_http(|http| http.bind(addr.to_string()))
                // Bounded, because establishment has to give up on its own --
                // but not tight: this budget covers the `connect` before the
                // subscription as well, and a loaded machine makes a short one
                // flake. The assertion below waits longer still.
                .with_timeout(std::time::Duration::from_secs(5))
        });
        client.connect().await.expect("connect");

        client
            .listen(crate::types::SubscriptionFilter::new().with_tools_changed())
            .await
            .expect_err("a stalled subscription must not establish");

        // Giving up has to reach the wire: the peer sees the connection go away
        // instead of holding a request nobody is waiting for.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while !closed.load(AtomicOrdering::SeqCst) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(
            closed.load(AtomicOrdering::SeqCst),
            "abandoning an unacknowledged subscription must close its request"
        );
    }

    /// A rejected `subscriptions/listen` must surface the server's own error
    /// immediately. Waiting only on the acknowledgment would sit out the full
    /// request timeout and report *that* instead -- the peer's reason lost, and
    /// the caller blocked for no reason.
    #[tokio::test(flavor = "multi_thread")]
    async fn listen_surfaces_an_immediate_rejection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_rejecting(listener));

        let mut client = Client::new().with_options(|opt| {
            opt.with_http(|http| http.bind(addr.to_string()))
                // Generous on purpose: a timeout would be unmistakable below.
                .with_timeout(std::time::Duration::from_secs(30))
        });
        client.connect().await.expect("connect");

        let started = tokio::time::Instant::now();
        let err = client
            .listen(crate::types::SubscriptionFilter::new().with_tools_changed())
            .await
            .expect_err("a rejected subscription must fail");

        let reported = format!("{err:?}");
        assert!(
            reported.contains(REASON),
            "the server's own error must survive, got: {reported}"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "the rejection must not wait out the request timeout"
        );
    }
}

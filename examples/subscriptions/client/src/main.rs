//! MCP 2026-07-28 example client for `subscriptions/listen`.
//!
//! Opens one long-lived subscription, triggers the mutations that produce the
//! notifications it asked for, and closes it. Notifications are dispatched to
//! the handlers registered before listening -- the [`Subscription`] handle is
//! about the stream's lifecycle, not its contents.

use neva::prelude::*;
use neva::types::SubscriptionFilter;
use std::time::Duration;
use tracing_subscriber::prelude::*;

const WATCHED: &str = "res://config";

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let mut client = Client::new().with_options(|opt| {
        opt.with_http(|http| http.bind("127.0.0.1:3000").with_endpoint("/mcp"))
            .with_timeout(Duration::from_secs(5))
    });

    client.connect().await?;

    // After `connect`: these helpers assert the server advertises the matching
    // capability, which is only known once it has been discovered.
    client.on_tools_changed(|_| async {
        tracing::info!("the tool list changed -- time to re-list");
    });
    client.on_resource_changed(|n| async move {
        tracing::info!("resource updated: {:?}", n.params);
    });

    // One stream, two notification types. `resourceSubscriptions` is where the
    // removed `resources/subscribe` RPC went.
    let mut subscription = client
        .listen(
            SubscriptionFilter::new()
                .with_tools_changed()
                .with_resource(WATCHED),
        )
        .await?;

    tracing::info!("subscription {} established", subscription.id());
    tracing::info!("accepted filter: {:?}", subscription.acknowledged());
    if !subscription.is_fully_honored() {
        tracing::warn!("the server narrowed the filter; some types will never arrive");
    }

    // Both calls mutate server state, and both notifications come back on the
    // subscription rather than on the call's own reply.
    client.call_tool("publish", ("name", "greet")).await?;
    client.call_tool("touch", ()).await?;

    // The stream is live and independent of these calls, so give the
    // notifications a moment to land before tearing it down.
    tokio::time::sleep(Duration::from_millis(500)).await;

    subscription.cancel().await?;
    tracing::info!("subscription ended: {:?}", subscription.closed().await);

    client.disconnect().await
}

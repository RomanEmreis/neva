# Subscriptions Example -- MCP 2026-07-28

`subscriptions/listen` over **stateless HTTP**. One long-lived POST carries the
notifications the client opted in to, and nothing else.

The 2026-07-28 spec replaced the HTTP `GET` stream and the
`resources/subscribe` / `resources/unsubscribe` RPC pair with this single
request. A per-resource subscription did not disappear -- it became a URI in the
listen filter, scoped to the stream that carries it.

## Run

Two terminals, from this directory (`examples/subscriptions/`):

```bash
# terminal 1 -- start the server (binds 127.0.0.1:3000/mcp)
cargo run -p server
```

```bash
# terminal 2 -- run the client
cargo run -p client
```

## What to watch

The client opens one subscription for two categories -- tool-list changes and
updates to `res://config` -- then triggers both:

```
subscription 1 established
accepted filter: SubscriptionFilter { tools_list_changed: true, ... resource_subscriptions: [res://config] }
the tool list changed -- time to re-list
resource updated: {"_meta": {"io.modelcontextprotocol/subscriptionId": 1}, "uri": "res://config"}
subscription ended: Cancelled
```

Four things this shows:

* **The server writes no subscription code.** `subscriptions/listen` is handled
  by neva; the server only calls `ctx.add_tool` and `ctx.resource_updated`, and
  the notification reaches every stream whose filter admits it.
* **The accepted filter is narrowed to advertised capabilities.** Drop
  `with_list_changed()` from the server's options and the acknowledgment comes
  back without `toolsListChanged` -- the client learns immediately instead of
  waiting forever for a push that was never going to arrive.
* **Every message carries `io.modelcontextprotocol/subscriptionId`.** That is
  what lets a client demultiplex several subscriptions sharing one channel,
  which on stdio is always the case.
* **Cancelling closes the stream.** That is the spec's mechanism over HTTP, and
  the only sound one: a `notifications/cancelled` travels on its own POST and
  proves nothing about who opened the subscription. So the end state is
  `Cancelled` rather than `Graceful` -- there is no channel left for a final
  result, and none is expected.

Notifications are dispatched to the handlers registered *before* listening
(`on_tools_changed`, `on_resource_changed`); the `Subscription` handle is about
the stream's lifecycle -- what was accepted, cancelling it, and how it ended.

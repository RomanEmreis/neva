# MRTR (Multi Round-Trip Requests) Example — MCP 2026-07-28 RC

A new-spec checkout flow over **stateless HTTP**. The server's single
`place_order` tool elicits shipping details mid-handler and uses the MRTR effect
primitives so its side effects stay correct even though the handler re-runs from
the top on every round-trip.

## Run

Two terminals, from this directory (`examples/mrtr/`):

```bash
# terminal 1 — start the server (binds 127.0.0.1:3000/mcp)
cargo run -p server

# terminal 2 — run the client
cargo run -p client
```

The client prints `Order confirmed for Ada Lovelace shipping to 1 Analytical Way — total $12.99`.

## What to watch

On the **server** you'll see each of these logged **exactly once**, even though
`place_order` re-executes fully across the elicitation round-trip:

```
📦 fetching shipping quote…     # ctx.memo  — computed once, replayed after
💳 charging card…               # ctx.once  — effect guarded across rounds
✉️  receipt sent to Ada Lovelace # ctx.on_commit — runs once, on the final round
```

That once-only behavior is the whole point of `memo` / `once` / `on_commit`:
without them, the re-run model would re-fetch, double-charge, and re-send.

## New-spec mechanics shown here

- **Stateless HTTP** transport (no session id on the wire).
- **`server/discover`** instead of the `initialize`/`initialized` handshake
  (run automatically by `client.connect()`).
- **`ctx.elicit(key, params)`** — the two-arg, replay-aware elicitation API.
- **`requestState`** — an HMAC-signed blob the client echoes back, carrying the
  replay log plus `memo`/`once` bookkeeping. Set a shared signing key in
  production via `App::with_request_state_secret`.
- Proc macros: **`#[tool]`**, **`#[elicitation]`**, **`#[json_schema]`**.

Legacy push-model features (URL elicitation, `complete_elicitation`,
`on_elicitation_completed`) are intentionally not used — they don't apply to the
stateless model.

# Example of using LLM Sampling

Two variants, one per protocol generation.

## MCP 2026-07-28 -- default (`sampling/createMessage` as an MRTR input request)

MCP 2026-07-28 removed the capability-driven push request. The *ability* was
re-homed onto MRTR: the server borrows the client's model with
`ctx.sample(key, params)`, the reply is an `input_required` result carrying a
`sampling/createMessage` envelope, and the client echoes the completion back in
`inputResponses` alongside the opaque `requestState`. Sampling arrives
**deprecated** -- it exists for migration.

### Run the server
```
cargo run --manifest-path examples/sampling/server/Cargo.toml
```
### Run the client
```
cargo run --manifest-path examples/sampling/client/Cargo.toml
```

The client prints `[o3-mini] Revenue grew 12% with steady churn, ...`.

### What to watch

The server logs show the whole point of the MRTR effect primitives:

```
📝 summarize_report round starting...   # round 1
📚 fetching source report...            # ctx.memo -- computed once
📝 summarize_report round starting...   # round 2: the handler re-runs...
🗄️  summary archived                  # ctx.on_commit -- fires once, at the end
```

`📚 fetching source report...` appears **once** even though the handler ran
**twice**: the memo value is replayed out of `requestState` on the second round.

The handler is wired with an explicit `client.map_sampling(...)` rather than the
`#[sampling]` attribute -- that macro belongs to the legacy push model and is not
available in the default (MCP 2026-07-28) build.

## Legacy (capability + `sampling/createMessage` push request)

Built with the `legacy-spec` feature. The server calls `ctx.sample(params)` and
the client answers a real `sampling/createMessage` request pushed over the
session. This variant also shows the tool-use loop over TLS with JWT auth.

### Run the server
```
JWT_SECRET=a-string-secret-at-least-256-bits-long cargo run --manifest-path examples/sampling/legacy/server/Cargo.toml
```

### Run the client
```
cargo run --manifest-path examples/sampling/legacy/client/Cargo.toml
```

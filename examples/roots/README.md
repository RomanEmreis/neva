# Example of using Roots

Two variants, one per protocol generation.

## MCP 2026-07-28 -- default (`roots/list` as an MRTR input request)

MCP 2026-07-28 removed the capability-driven push request. The *ability* was
re-homed onto MRTR: the server asks with `ctx.list_roots(key)`, the reply is an
`input_required` result carrying a `roots/list` envelope, and the client echoes
its roots back in `inputResponses` alongside the opaque `requestState`. Roots
arrive **deprecated** -- they exist for migration.

### Run the server
```
cargo run --manifest-path examples/roots/server/Cargo.toml
```
### Run the client
```
cargo run --manifest-path examples/roots/client/Cargo.toml
```

The client prints `2 root(s): My Project (...), My Another Project (...)`.

### What to watch

`🔎 scan_workspace round starting...` is logged **twice** on the server: the
handler re-runs from the top on the second round, with the roots replayed out
of `requestState` instead of asked for again. Anything expensive above an input
point belongs in `ctx.memo` / `ctx.once` for that reason -- see
[`examples/mrtr`](../mrtr) for the effect primitives.

The client never registers a roots *handler*: roots are configured data, so
having any is what makes it declare `clientCapabilities.roots`.

## Legacy (capability + `roots/list` push request)

Built with the `legacy-spec` feature. The server calls `ctx.list_roots()` and
the client answers a real `roots/list` request pushed over the session.

### Run the server
```
cargo run --manifest-path examples/roots/legacy/server/Cargo.toml
```
### Run the client
```
cargo run --manifest-path examples/roots/legacy/client/Cargo.toml
```

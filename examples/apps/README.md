# Example of using MCP Apps

A tool with a face: a host that supports
[MCP Apps](https://github.com/modelcontextprotocol/ext-apps) renders `get_time`
with the `ui://clock/app.html` document the server also serves. The tool checks
`ctx.supports_apps()` and answers with the bare time for a caller that renders
it, and with a full sentence for one that does not.

The server never sends or receives a `ui/*` message -- that traffic runs between
the host and the iframe. It serves a tool and an HTML document; the host does the
theater.

## Run the client

Spawns the server itself.

```
cargo run --manifest-path examples/apps/client/Cargo.toml
```

## Run the server under the inspector

```
npx @modelcontextprotocol/inspector cargo run --manifest-path examples/apps/server/Cargo.toml
```

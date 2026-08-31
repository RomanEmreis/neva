# Example of using MCP Apps

A tool with a face: `get_time` returns a sentence, and a host that supports
[MCP Apps](https://github.com/modelcontextprotocol/ext-apps) renders it with the
`ui://clock/app.html` document the server also serves.

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

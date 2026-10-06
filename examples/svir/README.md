# Handing MCP tools to a model -- the svir bridge

A task board written once, with `#[tool]` and `#[prompt]`, and handed to a model
through [svir](https://docs.rs/svir) two ways: served over MCP and called from a
client, or called in the same process with no MCP in between.

Any OpenAI-compatible model server will do. `SVIR_URL` names it,
`http://127.0.0.1:1234` (LM Studio) unless set; `SVIR_MODEL` names the model, and
it should be one that can call tools.

## Run

Over MCP, two terminals, from this directory (`examples/svir/`):

```bash
# terminal 1 -- the task board, served at 127.0.0.1:3000/mcp
cargo run -p server

# terminal 2 -- its tools and prompt, handed to a model
SVIR_MODEL=<model> cargo run -p client
```

In one process, with no MCP transport:

```bash
SVIR_MODEL=<model> cargo run -p server -- --local
```

Either way the model's tool calls are logged as they come (`-- add_task(...)`),
and its answer is printed at the end.

## What it shows

- **`RemoteTools`** (client): a connected server's tools as a `svir::Toolbox`.
  `prefixed("board_")` keeps them apart from any other server's in one request;
  the calls of one turn run concurrently over the shared client.
- **`prompt_messages`** (client): the server's `plan_week` prompt opens the
  conversation, as svir messages.
- **`App::into_toolbox`** (server, `--local`): the same `#[tool]` functions
  called in this process, through the server's own pipeline, with `Dc<Board>`
  injected as over MCP.

`App::with_toolbox` does both at once: the server keeps serving over MCP while
the toolbox calls its tools in-process.

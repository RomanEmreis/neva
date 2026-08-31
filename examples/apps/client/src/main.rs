//! The other half of the negotiation: a client that declares MCP Apps and reads
//! back what the server offers.
//!
//! Run with:
//!
//! ```no_rust
//! cargo run --manifest-path examples/apps/client/Cargo.toml
//! ```
//!
//! A neva client is not a browser, so it does not render anything -- the `ui/*`
//! traffic runs between a host and its iframe. What it does is the part a host
//! needs from an MCP library: declare the extension, find which tools have a
//! face, fetch the HTML, and know which tools the model may see.

use neva::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let mut client = Client::new().with_options(|opt| {
        // Spawns the server the way the other paired examples do. The parent
        // `cargo run` has already built the shared dependency tree into this
        // workspace's target directory, so the child only compiles the server
        // itself and finishes well inside the request timeout.
        opt.with_stdio(
            "cargo",
            ["run", "--manifest-path", "examples/apps/server/Cargo.toml"],
        )
            // The client half: advertises `io.modelcontextprotocol/ui` with
            // `mimeTypes: ["text/html;profile=mcp-app"]`. A server checks this
            // before offering a UI-bound tool instead of a text-only one.
            .with_apps()
    });

    client.connect().await?;

    let tools = client.list_tools(None).await?;

    for tool in tools.tools.iter() {
        // Every tool has a `content` answer; only some have a face.
        let Some(ui) = tool.ui() else {
            println!("{}: no UI", tool.name);
            continue;
        };

        // A host MUST NOT put this in the agent's tool list. Filtering is the
        // host's job -- the server lists app-only tools like any other.
        let audience = if tool.is_model_visible() {
            "model + app"
        } else {
            "app only"
        };
        println!("{}: {} -> {:?}", tool.name, audience, ui.resource_uri);
    }

    // Fetch the document behind one of them. This is the `resources/read` a host
    // makes before it opens an iframe.
    if let Some(uri) = tools
        .get("get_time")
        .and_then(|tool| tool.ui())
        .and_then(|ui| ui.resource_uri)
    {
        let result = client.read_resource(uri).await?;
        for contents in result.contents.iter() {
            println!(
                "\n{} [{}] {} bytes",
                contents.uri(),
                contents.mime().unwrap_or("?"),
                contents.text().map(str::len).unwrap_or_default()
            );
            // The security block the host turns into a CSP and an `allow`
            // attribute. Absent means the restrictive default: no external
            // access of any kind.
            println!("  _meta.ui: {:?}", contents.ui());
        }
    }

    client.disconnect().await
}

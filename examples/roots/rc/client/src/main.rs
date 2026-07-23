//! New-spec (MCP 2026-07-28 RC) roots example client.
//!
//! Roots are configured *data*, not a handler: the client answers the server's
//! MRTR `roots/list` input request from the list it was built with. Because the
//! list is non-empty, the client automatically declares
//! `clientCapabilities.roots` on every request -- a server may only ask for a
//! kind the client declared.
//!
//! The MRTR round-trips happen inside `call_tool`: the caller sees one call.

use neva::prelude::*;
use tracing_subscriber::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    let mut client = Client::new().with_options(|opt| {
        opt.with_http(|http| http.bind("127.0.0.1:3001").with_endpoint("/mcp"))
    });

    // Deprecated on arrival, like the whole roots kind -- the API stays for
    // migration.
    #[allow(deprecated)]
    client
        .add_root("file:///home/user/projects/my_project", "My Project")
        .add_root(
            "file:///home/user/projects/my_another_project",
            "My Another Project",
        );

    // `connect()` runs `server/discover` -- no `initialize` handshake under RC.
    client.connect().await?;

    let result = client.call_tool("scan_workspace", ()).await?;
    tracing::info!("Result: {:?}", result.content);

    client.disconnect().await
}

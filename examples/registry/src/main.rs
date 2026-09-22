//! Publishing a neva server to the [MCP Registry](https://registry.modelcontextprotocol.io).
//!
//! The registry lists servers so clients can find and install them. What it
//! lists is a `server.json`, and most of what goes in one this server already
//! knows -- its version, its transport, the crate Cargo is building -- so it is
//! written out rather than typed by hand.
//!
//! Run the server as usual:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector cargo run -p example-registry
//! ```
//!
//! ...or ask it for its manifest:
//!
//! ```no_rust
//! cargo run -p example-registry -- --emit-manifest > server.json
//! ```
//!
//! # Publishing your own
//!
//! Not this crate: it is `publish = false`, as every example here is, and a
//! listing for `example-registry` would be nobody's server. What follows is the
//! path for the crate you copy this into.
//!
//! 1. `cargo publish`, so there is something to install.
//! 2. Put the server name in that crate's README as **visible text**, which is
//!    how crates.io packages prove ownership:
//!
//!    ```no_rust
//!    - MCP Registry name: `mcp-name: io.github.<your-user>/<your-server>`
//!    ```
//!
//!    Not as an HTML comment: crates.io strips those when it renders markdown,
//!    and the validator reads the rendered HTML. (PyPI and NuGet keep comments;
//!    cargo is the exception.)
//! 3. Write the manifest, prove the namespace is yours, and publish it:
//!
//!    ```no_rust
//!    cargo run -p <your-server> -- --emit-manifest > server.json
//!    mcp-publisher login github
//!    mcp-publisher publish
//!    ```
//!
//! `login github` is what makes `io.github.<user>/...` yours to publish under.
//! A domain namespace (`com.example/...`) is proved with DNS instead.
//!
//! `mcp-publisher init` writes a skeleton too, and for a Rust crate it writes
//! the wrong one: it picks the package type by looking for `package.json`,
//! `pyproject.toml` or a `Dockerfile`, and falls back to npm -- so a crate
//! comes out as `"registryType": "npm"` with a placeholder identifier. This
//! path also keeps the version in step on every release, which a file written
//! once and edited by hand does not.
//!
//! # The one thing that is not derived
//!
//! The registry's name and the MCP server name are different strings under
//! different rules. `with_name("weather")` below is the MCP one -- what
//! `serverInfo` carries and what a user sees in a client. The registry's is a
//! reverse-DNS identifier inside a namespace you have proved you own, and
//! passing one where the other belongs produces a manifest that fails namespace
//! verification. So it is written out here, in full, once.

use neva::prelude::*;
use neva::registry::KeyValueInput;

/// The registry identifier this server publishes under. Yours, not this one:
/// `io.github.<your-user>/<your-server>`.
const REGISTRY_NAME: &str = "io.github.romanemreis/weather";

#[tool(descr = "Returns the forecast for a city")]
fn forecast(city: String) -> String {
    format!("Sunny in {city}, as ever")
}

#[tokio::main]
async fn main() {
    let app = App::new().with_options(|opt| {
        opt.with_stdio()
            // The MCP server name, and the version the manifest publishes.
            .with_name("weather")
            .with_version(env!("CARGO_PKG_VERSION"))
    });

    if std::env::args().any(|arg| arg == "--emit-manifest") {
        emit_manifest(&app);
        return;
    }

    app.run().await;
}

/// Writes the manifest to stdout, or says what is missing and exits non-zero --
/// a build that cannot produce a valid `server.json` should fail where it is
/// built, not at the upload.
fn emit_manifest(app: &App) {
    // `cargo_env!()` reads the `CARGO_PKG_*` of *this* crate: its name and
    // version become the cargo package entry, its description and repository
    // become the listing. A macro because those are compile-time values here --
    // a function inside neva would read neva's own.
    let manifest = app
        .server_manifest(REGISTRY_NAME)
        .with_title("Weather")
        .with_cargo_package(neva::cargo_env!(), |package| {
            package.with_environment_variable(
                KeyValueInput::new("WEATHER_API_KEY")
                    .with_description("API key for the weather provider")
                    .required()
                    .secret(),
            )
        });

    // With nothing to add to the package, the whole of the above is one call:
    //
    //     let manifest = neva::server_manifest!(app, REGISTRY_NAME);

    match manifest.to_json() {
        Ok(json) => print!("{json}"),
        Err(err) => {
            // The limits are the schema's, and they are not Cargo's: a
            // description over 100 characters is the one that catches people,
            // because crates.io has no such ceiling.
            eprintln!("this server.json is not the shape the schema asks for: {err}");
            std::process::exit(1);
        }
    }
}

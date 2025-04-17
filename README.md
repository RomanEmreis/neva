# Neva
Easy configurable MCP server SDK for Rust

[![latest](https://img.shields.io/badge/latest-0.0.3-d8eb34)](https://crates.io/crates/neva)
[![latest](https://img.shields.io/badge/rustc-1.80+-964B00)](https://crates.io/crates/neva)
[![License: MIT](https://img.shields.io/badge/License-MIT-624bd1.svg)](https://github.com/RomanEmreis/volga/blob/main/LICENSE)
[![CI](https://github.com/RomanEmreis/neva/actions/workflows/rust.yml/badge.svg)](https://github.com/RomanEmreis/neva/actions/workflows/rust.yml)
[![Release](https://github.com/RomanEmreis/neva/actions/workflows/release.yml/badge.svg)](https://github.com/RomanEmreis/neva/actions/workflows/release.yml)

> 💡 **Note**: This project is currently in preview. Breaking changes can be introduced without prior notice.

[API Docs](https://docs.rs/neva/latest/neva/) | [Examples](https://github.com/RomanEmreis/neva/tree/main/examples)

## Dependencies
```toml
[dependencies]
neva = { version = "0.0.3", features = ["full"] }
tokio = { version = "1", features = ["full"] }
```

## Code

```rust
use neva::{App, tool, resource, prompt};

#[tool(descr = "A say hello tool")]
async fn hello(name: String) -> String {
    format!("Hello, {name}!")
}

#[resource(uri = "res://{name}", descr = "Some details about resource")]
async fn get_res(name: String) -> (String, String) {
    (
        format!("res://{name}"),
        format!("Some details about resource: {name}")
    )
}

#[prompt(descr = "Analyze code for potential improvements")]
async fn analyze_code(lang: String) -> (String, String) {
    (format!("Language: {lang}"), "user".into())
}

#[tokio::main]
async fn main() {
    let mut app = App::new()
        .with_options(|opt| opt
            .with_stdio()
            .with_name("Sample MCP server")
            .with_version("1.0.0"));

    map_hello(&mut app);
    map_get_res(&mut app);
    map_analyze_code(&mut app);

    app.run().await;
}
```

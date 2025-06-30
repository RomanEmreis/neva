# Neva
Easy configurable MCP server and client SDK for Rust

[![latest](https://img.shields.io/badge/latest-0.1.2-d8eb34)](https://crates.io/crates/neva)
[![latest](https://img.shields.io/badge/rustc-1.85+-964B00)](https://crates.io/crates/neva)
[![License: MIT](https://img.shields.io/badge/License-MIT-624bd1.svg)](https://github.com/RomanEmreis/neva/blob/main/LICENSE)
[![CI](https://github.com/RomanEmreis/neva/actions/workflows/rust.yml/badge.svg)](https://github.com/RomanEmreis/neva/actions/workflows/rust.yml)
[![Release](https://github.com/RomanEmreis/neva/actions/workflows/release.yml/badge.svg)](https://github.com/RomanEmreis/neva/actions/workflows/release.yml)

> 💡 **Note**: This project is currently in preview. Breaking changes can be introduced without prior notice.

[API Docs](https://docs.rs/neva/latest/neva/) | [Examples](https://github.com/RomanEmreis/neva/tree/main/examples)

## MCP Client

### Dependencies
```toml
[dependencies]
neva = { version = "0.1.2", features = ["client-full"] }
tokio = { version = "1", features = ["full"] }
```

### Code
```rust
use std::time::Duration;
use neva::Client;
use neva::error::Error;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let mut client = Client::new()
        .with_options(|opt| opt
            .with_stdio("npx", ["-y", "@modelcontextprotocol/server-everything"])
            .with_timeout(Duration::from_secs(5)));
    
    client.connect().await?;

    // List tools
    let tools = client.list_tools(None).await?;
    for tool in tools.tools {
        println!("- {}", tool.name);
    }

    // Call a tool
    let args = [
        ("message", "Hello MCP!")
    ];
    let result = client.call_tool("echo", Some(args)).await?;
    println!("{:?}", result.content);

    client.disconnect().await
}
```

## MCP Server

### Dependencies
```toml
[dependencies]
neva = { version = "0.1.2", features = ["server-full"] }
tokio = { version = "1", features = ["full"] }
```

### Code
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
    App::new()
        .with_options(|opt| opt
            .with_stdio()
            .with_name("Sample MCP server")
            .with_version("1.0.0"))
        .run()
        .await;
}
```

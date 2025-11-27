# Neva

Blazingly fast and easily configurable [Model Context Protocol (MCP)](https://modelcontextprotocol.io) server and client SDK for Rust.
With simple configuration and ergonomic APIs, it provides everything you need to quickly build MCP clients and servers, 
fully aligned with the latest MCP specification.

[![latest](https://img.shields.io/badge/latest-0.2.4-d8eb34)](https://crates.io/crates/neva)
[![latest](https://img.shields.io/badge/rustc-1.90+-964B00)](https://crates.io/crates/neva)
[![License: MIT](https://img.shields.io/badge/License-MIT-624bd1.svg)](https://github.com/RomanEmreis/neva/blob/main/LICENSE)
[![CI](https://github.com/RomanEmreis/neva/actions/workflows/rust.yml/badge.svg)](https://github.com/RomanEmreis/neva/actions/workflows/rust.yml)
[![Release](https://github.com/RomanEmreis/neva/actions/workflows/release.yml/badge.svg)](https://github.com/RomanEmreis/neva/actions/workflows/release.yml)

> 💡 **Note**: This project is currently in preview. Breaking changes can be introduced without prior notice.

[Tutorial](https://romanemreis.github.io/neva-docs/) | [API Docs](https://docs.rs/neva/latest/neva/) | [Examples](https://github.com/RomanEmreis/neva/tree/main/examples)

## Key Features
- **Client & Server SDK** - one library to build both MCP clients and servers with the powers of Rust.
- **Performance** - asynchronous and Tokio-powered.
- **Transports** - **stdio** for local integrations and **Streamable HTTP** for remote, bidirectional communication.
- **Tools**, **Resources** & **Prompts** - full-house support for defining and consuming the main MCP entities.
- **Authentication & Authorization** - bearer token authentication, role-based access control, and more to fit high security standards.
- **Structured Data** - output validation, embedded resources, and resource links out of the box.
- **Spec Alignment** - designed to track the latest MCP specification and cover its core functionality.

## Quick Start
### MCP Client
#### Dependencies
```toml
[dependencies]
neva = { version = "0.2.4", features = ["client-full"] }
tokio = { version = "1", features = ["full"] }
```

#### Code
```rust
use neva::prelude::*;

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
    let result = client.call_tool("echo", args).await?;
    println!("{:?}", result.content);

    client.disconnect().await
}
```

### MCP Server
#### Dependencies
```toml
[dependencies]
neva = { version = "0.2.4", features = ["server-full"] }
tokio = { version = "1", features = ["full"] }
```
#### Code
```rust
use neva::prelude::*;

#[tool(descr = "A say hello tool")]
async fn hello(name: String) -> String {
    format!("Hello, {name}!")
}

#[resource(uri = "res://{name}", descr = "Some details about resource")]
async fn get_res(name: String) -> ResourceContents {
    ResourceContents::new(format!("res://{name}"))
        .with_mime("plain/text")
        .with_text(format!("Some details about resource: {name}"))
}

#[prompt(descr = "Analyze code for potential improvements")]
async fn analyze_code(lang: String) -> PromptMessage {
    PromptMessage::user()
        .with(format!("Language: {lang}"))
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

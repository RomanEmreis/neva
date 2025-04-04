# Neva
Easy configurable MCP server SDK for Rust

---

## Dependencies
```toml
[dependencies]
neva = "0.0.1"
tokio = { version = "1", features = ["full"] }
```

## Code

```rust
use neva::App;
use neva_macros::{tool, resource, prompt};

#[tool(descr = "A say hello tool")]
async fn hello(name: String) -> String {
    format!("Hello, {name}!")
}

#[tokio::main]
async fn main() {
    let mut app = App::new()
        .with_options(|opt| opt
            .with_stdio()
            .with_name("Sample MCP server")
            .with_version("1.0.0"));

    map_hello(&mut app);

    app.run().await;
}
```

# Neva
Easy configurable MCP server SDK for Rust

## Dependencies
```toml
[dependencies]
neva = "0.0.1"
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

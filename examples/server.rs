﻿//! Run with:
//! 
//! ```no_rust
//! npx @modelcontextprotocol/inspector cargo run --example server
//! ```

use neva::{App, types::Json};

#[tokio::main]
async fn main() {
    let mut app = App::new()
        .with_options(|opt| opt.with_stdio());

    app.map_tool("say_hello", || async {
        "Hello, world!"
    });
    
    app.map_tool("say_hello_to", |name: String| async move {
       format!("Hello, {name}!")
    });

    app.map_tool("say_json", |arg: Json<Payload>| async move {
        format!("{}, {}!", arg.say, arg.name)
    });

    app.map_tool("v2/say_json", |arg: serde_json::Value| async move {
        arg
    });
    
    app.run().await;
}

#[derive(serde::Deserialize)]
struct Payload {
    say: String,
    name: String,
}
//! Run with:
//! 
//! ```no_rust
//! npx @modelcontextprotocol/inspector cargo run --example server
//! ```

use neva::App;

#[tokio::main]
async fn main() {
    let mut app = App::new();

    app.map_tool("say_hello", || async {
        "Hello, world!"
    });
    
    app.map_tool("say_hello_to", |name: String| async move {
       format!("Hello, {name}!")
    });
    
    app.run().await;
}
//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector cargo run -p middlewares
//! ```

use neva::prelude::*;
use tracing_subscriber::{prelude::*, filter, reload};

#[tool(middleware = [specific_middleware])]
async fn greeter(name: String) -> String {
    format!("Hello, {}!", name)
}

#[tool]
async fn hello_world() -> &'static str {
    "Hello, World!"
}

#[resource(uri = "res://{name}")]
async fn resource(name: String) -> ResourceContents {
    ResourceContents::new(name)
        .with_text("Hello, world!")
}

#[prompt(middleware = [specific_middleware])]
async fn prompt(topic: String) -> PromptMessage {
    PromptMessage::user()
        .with(format!("Sample prompt of {topic}"))
}

#[prompt]
async fn another_prompt(topic: String) -> PromptMessage {
    PromptMessage::user()
        .with(format!("Another sample prompt of {topic}"))
}

#[handler(command = "ping", middleware = [specific_middleware])]
async fn ping_handler() {
    eprintln!("pong");
}

async fn logging_middleware(ctx: MwContext, next: Next) -> Response {
    let id = ctx.id();
    tracing::info!("Request start: {id:?}");
    
    let resp = next(ctx).await;
    
    tracing::info!("Request end: {id:?}");
    resp
}

async fn global_tool_middleware(ctx: MwContext, next: Next) -> Response {
    tracing::info!("Tool called");
    next(ctx).await
}

// Wraps all requests for the "greeter" tool, "prompt" prompt and ping handler 
async fn specific_middleware(ctx: MwContext, next: Next) -> Response {
    tracing::info!("Hello from specific middleware");
    next(ctx).await
}

#[tokio::main]
async fn main() {
    let (filter, handle) = reload::Layer::new(filter::LevelFilter::DEBUG);
    tracing_subscriber::registry()
        .with(filter) 
        .with(tracing_subscriber::fmt::layer()
            .event_format(notification::NotificationFormatter))
        .init();
    
    App::new()
        .with_options(|opt| opt
            .with_stdio()
            .with_logging(handle))
        .wrap(logging_middleware)           // Wraps all requests that pass through the server
        .wrap_tools(global_tool_middleware) // Wraps all tools/call requests that pass through the server
        .run()
        .await;
}

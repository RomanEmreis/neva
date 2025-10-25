//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector cargo run -p middlewares
//! ```

use neva::prelude::*;
use tracing_subscriber::{prelude::*, filter, reload};

#[tool(middleware = [specific_tool_middleware])]
async fn hello_world(name: String) -> String {
    format!("Hello, {}!", name)
}

#[tool]
async fn another_tool() -> &'static str {
    "Hello, World!"
}

#[resource(uri = "res://{name}")]
async fn resource(name: String) -> ResourceContents {
    ResourceContents::new(name)
        .with_text("Hello, world!")
}

#[prompt]
async fn prompt(topic: String) -> PromptMessage {
    PromptMessage::user()
        .with(format!("Sample prompt of {topic}"))
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

async fn specific_tool_middleware(ctx: MwContext, next: Next) -> Response {
    tracing::info!("Hello tool called");
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
        .with(logging_middleware)
        .with_tool(global_tool_middleware)
        .run()
        .await;
}

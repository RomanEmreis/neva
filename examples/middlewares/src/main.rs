//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector cargo run -p middlewares
//! ```

use neva::prelude::*;
use tracing_subscriber::{prelude::*, filter, reload};

#[tool]
async fn hello_world(name: String) -> String {
    format!("Hello, {}!", name)
}

async fn logging_middleware(ctx: MwContext, next: Next) -> Response {
    let id = ctx.id();
    tracing::info!("Request start: {id:?}");
    
    let resp = next(ctx).await;
    
    tracing::info!("Request end: {id:?}");
    resp
}

async fn filter_middleware(ctx: MwContext, next: Next) -> Response {
    if ctx.request().is_some_and(|c| c.params.is_none()) {
        return Response::error(ctx.id(), Error::new(
            ErrorCode::InvalidParams,
            "Request params are empty"
        ));
    }
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
        .with(filter_middleware)
        .run()
        .await;
}

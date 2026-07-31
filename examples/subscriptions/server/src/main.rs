//! MCP 2026-07-28 example server for `subscriptions/listen`.
//!
//! Nothing here handles the subscription: neva answers `subscriptions/listen`
//! itself, narrowing the client's filter to the capabilities configured below.
//! A server just mutates its own state -- `ctx.add_tool`, `ctx.resource_updated`
//! -- and the notification reaches every stream that asked for it.

use neva::prelude::*;
use tracing_subscriber::prelude::*;

const WATCHED: &str = "res://config";

/// Adds a tool, which emits `notifications/tools/list_changed`.
#[tool]
async fn publish(mut ctx: Context, name: String) -> Result<String, Error> {
    ctx.add_tool(Tool::new(name.clone(), || async { "hello" }))
        .await?;
    tracing::info!("published tool `{name}`");
    Ok(format!("published `{name}`"))
}

/// Marks the watched resource dirty, which emits
/// `notifications/resources/updated` -- but only to the streams whose filter
/// lists this URI.
#[tool]
async fn touch(mut ctx: Context) -> Result<String, Error> {
    ctx.resource_updated(WATCHED).await?;
    tracing::info!("touched {WATCHED}");
    Ok(format!("touched {WATCHED}"))
}

#[resource(uri = "res://config")]
async fn config() -> TextResourceContents {
    TextResourceContents::new(WATCHED, "{ \"answer\": 42 }")
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // The advertised capabilities are exactly what a listen filter can ask for:
    // a category the server does not announce is dropped from the accepted
    // filter rather than refused.
    App::new()
        .with_options(|opt| {
            opt.with_http(|http| http.bind("127.0.0.1:3000").with_endpoint("/mcp"))
                .with_tools(|tools| tools.with_list_changed())
                .with_resources(|res| res.with_list_changed().with_subscribe())
        })
        .run()
        .await;

    Ok(())
}

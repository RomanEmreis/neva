//! Batch example — server side
//!
//! Run with:
//! ```no_rust
//! cargo run -p server --manifest-path examples/batch/Cargo.toml
//! ```

use neva::prelude::*;

/// Adds two integers and returns the sum.
#[tool(descr = "Add two integers")]
async fn add(a: i32, b: i32) -> i32 {
    a + b
}

/// Returns a friendly greeting message for the given name.
#[prompt(
    descr = "Generate a greeting message",
    args = r#"[{"name": "name", "description": "Name to greet", "required": true}]"#
)]
async fn greeting(name: String) -> PromptMessage {
    PromptMessage::user().with(format!("Hello, {name}! Welcome to Neva."))
}

#[resources]
async fn list_notes(_params: ListResourcesRequestParams) -> impl Into<ListResourcesResult> {
    [
        Resource::new("notes://daily", "Daily notes")
            .with_descr("Today's notes")
            .with_mime("text/plain"),
        Resource::new("notes://weekly", "Weekly notes")
            .with_descr("This week's notes")
            .with_mime("text/plain"),
        Resource::new("notes://monthly", "Monthly notes")
            .with_descr("This month's notes")
            .with_mime("text/plain"),
    ]
}

#[resource(uri = "notes://daily", mime = "text/plain")]
async fn daily_notes() -> TextResourceContents {
    TextResourceContents::new("notes://daily", "- Finish JSON-RPC batch support\n- Write example\n- Update docs")
}

#[resource(uri = "notes://weekly", mime = "text/plain")]
async fn weekly_notes() -> TextResourceContents {
    TextResourceContents::new("notes://weekly", "- Batch API design\n- Implementation\n- Code review\n- Tests")
}

#[resource(uri = "notes://monthly", mime = "text/plain")]
async fn monthly_notes() -> TextResourceContents {
    TextResourceContents::new("notes://monthly", "- Neva v0.3 release\n- JSON-RPC batch\n- Task augmentation\n- Elicitation")
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    App::new()
        .with_options(|opt| opt
            .with_name("Batch Example Server")
            .with_default_http())
        .run()
        .await;
}

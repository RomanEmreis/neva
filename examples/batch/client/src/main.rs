//! Batch example — client side
//!
//! Run the server first, then:
//! ```no_rust
//! cargo run -p client --manifest-path examples/batch/Cargo.toml
//! ```

use neva::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    let mut client = Client::new()
        .with_options(|opt| opt.with_default_http());

    client.connect().await?;

    // Single batch with a tool call, resource reads, a prompt, and list requests

    tracing::info!("Sending batch request...");

    let responses = client
        .batch()
        .list_tools()
        .list_resources()
        .list_prompts()
        .call_tool("add", [("a", 40_i32), ("b", 2_i32)])
        .read_resource("notes://daily")
        .read_resource("notes://weekly")
        .get_prompt("greeting", [("name", "Neva")])
        .ping()
        .send()
        .await?;

    // Parse and log each response

    let [
        tools, 
        resources, 
        prompts, 
        add_result, 
        daily, 
        weekly, 
        greeting, 
        ping
    ] = responses.as_slice() else {
        return Err(Error::new(ErrorCode::InternalError, "unexpected number of responses"));
    };

    let tools = tools.clone().into_result::<ListToolsResult>()?;
    tracing::info!("Tools ({}):", tools.tools.len());
    for tool in &tools.tools {
        tracing::info!("  - {} : {}", tool.name, tool.descr.as_deref().unwrap_or(""));
    }

    let resources = resources.clone().into_result::<ListResourcesResult>()?;
    tracing::info!("Resources ({}):", resources.resources.len());
    for res in &resources.resources {
        tracing::info!("  - {} ({})", res.name, res.uri);
    }

    let prompts = prompts.clone().into_result::<ListPromptsResult>()?;
    tracing::info!("Prompts ({}):", prompts.prompts.len());
    for prompt in &prompts.prompts {
        tracing::info!("  - {} : {}", prompt.name, prompt.descr.as_deref().unwrap_or(""));
    }

    let add = add_result.clone().into_result::<CallToolResponse>()?;
    tracing::info!("add(40, 2) = {:?}", add.content);

    let daily_notes = daily.clone().into_result::<ReadResourceResult>()?;
    tracing::info!("notes://daily:\n{:?}", daily_notes.contents);

    let weekly_notes = weekly.clone().into_result::<ReadResourceResult>()?;
    tracing::info!("notes://weekly:\n{:?}", weekly_notes.contents);

    let greeting_result = greeting.clone().into_result::<GetPromptResult>()?;
    tracing::info!("greeting(\"Neva\"):\n{:?}", greeting_result.messages);

    tracing::info!("ping: {:?}", ping);

    client.disconnect().await
}

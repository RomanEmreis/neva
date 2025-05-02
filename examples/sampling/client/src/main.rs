use tracing_subscriber::prelude::*;
use neva::{
    Client, error::Error, 
    types::{
        Role, 
        sampling::{
            CreateMessageRequestParams, 
            CreateMessageResult
        }
    }
};

async fn sampling_handler(params: CreateMessageRequestParams) -> CreateMessageResult {
    let prompt: Vec<Option<String>> = params.messages
        .iter()
        .filter_map(|m| {
            if m.content.r#type == "text" {
                Some(m.content.text.clone())
            } else {
                None
            }
        })
        .collect();

    let sys_prompt = params
        .sys_prompt
        .unwrap_or_else(|| "You are a helpful assistant.".into());

    tracing::info!("Received prompt: {:?}, sys prompt: {:?}", prompt, sys_prompt);

    CreateMessageResult::new(Role::Assistant, "o3-mini", "Some response")
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    let mut client = Client::new()
        .with_options(|opt| opt
            .with_stdio("cargo", ["run", "--manifest-path", "examples/sampling/server/Cargo.toml"])
            .with_mcp_version("2024-11-05"));

    client.map_sampling(sampling_handler);

    client.connect().await?;

    let args = [("topic", "winter snow")];
    let result = client.call_tool("generate_poem", Some(args)).await?;
    tracing::info!("Received result: {:?}", result.content);

    client.disconnect().await
}

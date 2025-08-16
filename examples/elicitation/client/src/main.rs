use std::collections::HashMap;
use tracing_subscriber::prelude::*;
use neva::{
    Client, error::Error,
    types::elicitation::{ElicitRequestParams, ElicitationAction, ElicitResult}
};

async fn elicitation_handler(_params: ElicitRequestParams) -> ElicitResult {
    ElicitResult {
        action: ElicitationAction::Accept,
        content: Some(HashMap::from([
            ("mood".into(), "fun".into())
        ])),
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    let mut client = Client::new()
        .with_options(|opt| opt
            .with_stdio(
                "cargo", 
                ["run", "--manifest-path", "examples/elicitation/server/Cargo.toml"]));

    client.map_elicitation(elicitation_handler);

    client.connect().await?;

    let args = [("topic", "winter snow")];
    let result = client.call_tool("generate_poem", Some(args)).await?;
    tracing::info!("Received result: {:?}", result.content);

    client.disconnect().await
}

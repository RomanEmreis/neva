use tracing_subscriber::prelude::*;
use neva::{
    Client, error::Error,
    types::elicitation::{Validator, ElicitRequestParams, ElicitResult},
    elicitation, json_schema,
};

#[json_schema(ser)]
struct Contact {
    name: String,
    email: String,
    age: u32,
}

#[elicitation]
async fn elicitation_handler(params: ElicitRequestParams) -> ElicitResult {
    let contact = Contact {
        name: "John".to_string(),
        email: "john@email.com".to_string(),
        age: 30,
    };
    Validator::new(params)
        .validate(contact)
        .into()
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

    client.connect().await?;

    let result = client.call_tool::<[(&str, &str); 1], _>("generate_business_card", None).await?;
    tracing::info!("Received result: {:?}", result.content);

    client.disconnect().await
}

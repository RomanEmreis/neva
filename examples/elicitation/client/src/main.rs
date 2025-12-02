use tracing_subscriber::prelude::*;
use neva::prelude::*;

#[json_schema(ser)]
struct Contact {
    name: String,
    email: String,
    age: u32,
}

#[elicitation]
async fn elicitation_handler(params: ElicitRequestParams) -> ElicitResult {
    match params {
        ElicitRequestParams::Url(_url) => ElicitResult::accept()
            .with_content("Payments processed successfully."),
        ElicitRequestParams::Form(form) => {
            let contact = Contact {
                name: "John".to_string(),
                email: "john@email.com".to_string(),
                age: 30,
            };
            elicitation::Validator::new(form)
                .validate(contact)
                .into()  
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    let mut client = Client::new()
        .with_options(|opt| opt
            .with_elicitation(|e| e.with_form())
            .with_stdio(
                "cargo", 
                ["run", "--manifest-path", "examples/elicitation/server/Cargo.toml"]));

    client.connect().await?;

    let result = client.call_tool("generate_business_card", ()).await?;
    tracing::info!("Received result: {:?}", result.content);

    let result = client.call_tool("pay_a_bill", ()).await?;
    tracing::info!("Received result: {:?}", result.content);

    client.disconnect().await
}

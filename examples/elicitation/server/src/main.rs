use neva::{
    App, Context, error::Error,
    types::elicitation::{ElicitRequestParams},
    json_schema, tool
};

#[json_schema(de)]
struct Contact {
    name: String,
    email: String,
    age: u32,
}

#[tool]
async fn generate_business_card(mut ctx: Context) -> Result<String, Error> {
    let params = ElicitRequestParams::new("Please provide your contact information")
        .with_schema::<Contact>();
    
    ctx
        .elicit(params)
        .await?
        .map(format_contact)
}

fn format_contact(c: Contact) -> String {
    format!("Name: {}, Age: {}, email: {}", c.name, c.age, c.email)
}

#[tokio::main]
async fn main() {
    App::new()
        .with_options(|opt| opt.with_stdio())
        .run().await;
}

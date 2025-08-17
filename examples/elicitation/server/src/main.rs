use serde::Deserialize;
use schemars::JsonSchema;
use neva::{
    App, Context, error::{ErrorCode, Error},
    types::elicitation::{ElicitRequestParams},
    tool
};

#[derive(Debug, JsonSchema, Deserialize)]
struct Contact {
    name: String,
    email: String,
    age: u32,
}

#[tool]
async fn generate_business_card(mut ctx: Context) -> Result<String, Error> {
    let params = ElicitRequestParams::new("Please provide your contact information")
        .with_schema::<Contact>();
    
    let result = ctx.elicit(params).await?;

    if result.is_accepted() {
        let content = result.content::<Contact>().unwrap();
        Ok(format!("Name: {}, Age: {}, email: {}", content.name, content.age, content.email))   
    } else { 
        Err(Error::new(ErrorCode::InvalidRequest, "User rejected the request"))
    }
}

#[tokio::main]
async fn main() {
    App::new()
        .with_options(|opt| opt.with_stdio())
        .run().await;
}

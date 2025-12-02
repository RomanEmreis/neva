use neva::prelude::*;

#[json_schema(de)]
struct Contact {
    name: String,
    email: String,
    age: u32,
}

#[tool]
async fn generate_business_card(mut ctx: Context) -> Result<String, Error> {
    let params = ElicitRequestParams::form("Please provide your contact information")
        .with_schema::<Contact>();
    
    ctx
        .elicit(params.into())
        .await?
        .map(format_contact)
}

#[tool]
async fn pay_a_bill(mut ctx: Context) -> Result<String, Error> {
    let params = ElicitRequestParams::url(
        "https://www.paypal.com/us/webapps/mpp/paypal-payment", 
        "Please pay your bill using PayPal.");

    ctx
        .elicit(params.into())
        .await?;
    
    Ok("Payment successful".to_string())
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

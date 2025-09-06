use neva::{
    App, Context, error::Error, 
    types::sampling::CreateMessageRequestParams,
    tool
};

#[tool]
async fn generate_poem(mut ctx: Context, topic: String) -> Result<String, Error> {
    let params = CreateMessageRequestParams::new()
        .with_message(format!("Write a short poem about {topic}"))
        .with_sys_prompt("You are a talented poet who writes concise, evocative verses.");
    
    let result = ctx.sample(params).await?;
    Ok(format!("{:?}", result.content))
}

#[tokio::main]
async fn main() {
    let secret = std::env::var("JWT_SECRET")
        .expect("JWT_SECRET must be set");
    
    App::new()
        .with_options(|opt| opt
            .with_http(|http| http
                .with_auth(|auth| auth
                    .validate_exp(false)
                    .with_aud(["some aud"])
                    .with_iss(["some issuer"])
                    .set_decoding_key(secret.as_bytes()))))
        .run()
        .await;
}

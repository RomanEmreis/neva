use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use neva::prelude::*;

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
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();
    
    let secret = std::env::var("JWT_SECRET")
        .expect("JWT_SECRET must be set");
    
    let http = HttpServer::new("localhost:7878")
        .with_tls(|tls| tls
            .set_cert("examples/sampling/cert/dev-server.pem")
            .set_key("examples/sampling/cert/dev-server.key"))
        .with_auth(|auth| auth
            .validate_exp(false)
            .with_aud(["some aud"])
            .with_iss(["some issuer"])
            .set_decoding_key(secret.as_bytes()));
    
    App::new()
        .with_options(|opt| opt.set_http(http))
        .run()
        .await;
}

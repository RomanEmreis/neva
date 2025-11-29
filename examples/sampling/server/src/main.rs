use tracing_subscriber::prelude::*;
use neva::prelude::*;

#[tool]
async fn generate_weather_report(mut ctx: Context, city: String) -> Result<String, Error> {
    let prompt = ctx.prompt("weather", ("city", city)).await?;
    let msg = prompt.messages.into_iter().next().unwrap();
    
    let Some(tool) = ctx.find_tool("get_weather").await else {
        return Err(ErrorCode::MethodNotFound.into());
    }; 
    
    let params = CreateMessageRequestParams::new()
        .with_message(SamplingMessage::user().with(msg.content))
        .with_sys_prompt("You are a helpful assistant.")
        .with_tools([tool]);
    
    let result = ctx.sample(params).await?;
    if result.stop_reason.is_some_and(|r| r == "toolUse") { 
                
    }
    
    Ok(format!("{:?}", result.content))
}

#[tool]
async fn get_weather(_city: String) -> Json<Weather> {
    Json(Weather {
        temperature: 15.0,
        humidity: 60.0,
    })
}

#[prompt]
async fn weather(city: String) -> PromptMessage {
    PromptMessage::user()
        .with(format!("What is the weather in {city}?"))
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
            .with_dev_cert(DevCertMode::Auto))
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

#[json_schema(serde, debug)]
struct Weather {
    temperature: f32,
    humidity: f32,
}

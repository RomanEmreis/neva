use tracing_subscriber::prelude::*;
use neva::prelude::*;

#[tool]
async fn generate_weather_report(mut ctx: Context, city1: String, city2: String) -> Result<String, Error> {
    let Some(tool) = ctx.find_tool("get_weather").await else {
        return Err(ErrorCode::MethodNotFound.into());
    };

    let prompt = ctx.prompt("weather", [
        ("city1", city1),
        ("city2", city2)
    ]).await?;
    
    let msg = prompt.messages
        .into_iter()
        .next()
        .ok_or_else(|| Error::new(ErrorCode::InvalidParams, "No prompt message found in prompt response."))?;
    
    let mut params = CreateMessageRequestParams::new()
        .with_message(SamplingMessage::from(msg))
        .with_sys_prompt("You are a helpful assistant.")
        .with_tools([tool]);
    
    loop {
        let result = ctx.sample(params.clone()).await?;
        if result.stop_reason == Some(StopReason::ToolUse) {
            let tools: Vec<ToolUse> = result.tools()
                .cloned()
                .collect();

            let assistant_msg = tools
                .iter()
                .fold(SamplingMessage::assistant(), |msg, tool| msg.with(tool.clone()));

            let tool_results = ctx.use_tools(tools).await;

            let user_msg = tool_results
                .into_iter()
                .fold(SamplingMessage::user(), |msg, result| msg.with(result));

            params = params
                .with_message(assistant_msg)
                .with_message(user_msg)
                .with_tool_choice(ToolChoiceMode::None);
        } else {
            return Ok(format!("{:?}", result.content));
        };
    }
}

#[tool]
async fn get_weather(city: String) -> Json<Weather> {
    if city == "London" {
        Json(Weather {
            temperature: 15.0,
            humidity: 80.0,
        })
    } else {
        Json(Weather {
            temperature: 18.0,
            humidity: 65.0,
        })
    }
}

#[prompt]
async fn weather(city1: String, city2: String) -> PromptMessage {
    PromptMessage::user()
        .with(format!("What's the weather like in {city1} and {city2}?"))
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

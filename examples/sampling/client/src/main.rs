use tracing_subscriber::prelude::*;
use neva::prelude::*;

const ACCESS_TOKEN: &str =
    "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwiaXNzIjoic29tZSBpc3N1ZXIiLCJhdWQiOiJzb21lIGF1ZCIsImV4cCI6MH0.BYf42WI95BvIkpaXdTKKKvVtuVbcqQiZ1loXxSvNHBY";

#[sampling]
async fn sampling_handler(params: CreateMessageRequestParams) -> CreateMessageResult {
    tracing::info!("Received sampling: {:?}", params);
    
    if params.tool_choice.is_some_and(|c| !c.is_none()) {
        let prompts: Vec<String> = params.text()
            .flat_map(|c| c.as_text().map(|t| t.text.clone()))
            .collect();

        tracing::info!("Received prompts: {:?}", prompts);

        CreateMessageResult::assistant()
            .with_model("o3-mini")
            .use_tools([
                ("get_weather", ("city1", "London")),
                ("get_weather", ("city2", "Paris"))
            ])
    } else {
        let results: Vec<&ToolResult> = params.results()
            .flat_map(|c| c.as_result())
            .collect();
        
        tracing::info!("Received tool results: {results:?}");

        let response = 
            r#"Based on the current weather data:
                
               - **Paris**: 18°C and partly cloudy - quite pleasant!
               - **London**: 15°C and rainy - you'll want an umbrella.
               
               Paris has slightly warmer and drier conditions today."#;
        
        CreateMessageResult::assistant()
            .with_model("o3-mini")
            .with_content(response)
            .end_turn()
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    let mut client = Client::new()
        .with_options(|opt| opt
            .with_sampling(|s| s.with_tools())
            .with_http(|http| http
                .bind("localhost:7878")
                .with_tls(|tls| tls
                    .with_certs_verification(false))
                .with_auth(ACCESS_TOKEN)));
    
    client.connect().await?;

    let args = [
        ("city1", "London"),
        ("city2", "Paris"),
    ];
    let result = client
        .call_tool("generate_weather_report", args).await?;
    
    tracing::info!("Received result: {:?}", result.content);

    client.disconnect().await
}

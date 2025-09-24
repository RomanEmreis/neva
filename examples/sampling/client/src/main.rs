use tracing_subscriber::prelude::*;
use neva::prelude::*;

const ACCESS_TOKEN: &str =
    "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwiaXNzIjoic29tZSBpc3N1ZXIiLCJhdWQiOiJzb21lIGF1ZCIsImV4cCI6MH0.BYf42WI95BvIkpaXdTKKKvVtuVbcqQiZ1loXxSvNHBY";

#[sampling]
async fn sampling_handler(params: CreateMessageRequestParams) -> CreateMessageResult {
    let prompt: Vec<String> = params.text()
        .flat_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect();
    
    let sys_prompt = params
        .sys_prompt
        .unwrap_or_else(|| "You are a helpful assistant.".into());

    tracing::info!("Received prompt: {:?}, sys prompt: {:?}", prompt, sys_prompt);

    CreateMessageResult::assistant()
        .with_model("o3-mini")
        .with_content("Some response")
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    let mut client = Client::new()
        .with_options(|opt| opt
            .with_http(|http| http
                .bind("localhost:7878")
                .with_tls(|tls| tls.with_ca("examples/sampling/cert/dev-ca.pem"))
                .with_auth(ACCESS_TOKEN)));

    client.connect().await?;

    let args = ("topic", "winter snow");
    let result = client.call_tool("generate_poem", args).await?;
    tracing::info!("Received result: {:?}", result.content);

    client.disconnect().await
}

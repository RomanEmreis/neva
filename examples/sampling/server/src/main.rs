use neva::{
    App, Context, error::Error, 
    types::sampling::CreateMessageRequestParams,
    tool
};

#[tool]
async fn generate_poem(mut ctx: Context, topic: String) -> Result<String, Error> {
    let params = CreateMessageRequestParams::message(
        &format!("Write a short poem about {topic}"),
        "You are a talented poet who writes concise, evocative verses."
    );
    let result = ctx.sample(params).await?;
    Ok(format!("{:?}", result.content.text))
}

#[tokio::main]
async fn main() {
    App::new()
        .with_options(|opt| opt
            .with_stdio()
            .with_mcp_version("2024-11-05"))
        .run()
        .await;
}

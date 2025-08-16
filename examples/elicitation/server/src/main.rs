use neva::{
    App, Context, error::Error,
    types::elicitation::ElicitRequestParams,
    tool
};

#[tool]
async fn generate_poem(mut ctx: Context, _topic: String) -> Result<String, Error> {
    let params = ElicitRequestParams::new("What is the poem mood you'd like?")
        .with_required("mood", "string");
    let result = ctx.elicit(params).await?;
    Ok(format!("{:?}", result.content))
}

#[tokio::main]
async fn main() {
    App::new()
        .with_options(|opt| opt
            .with_stdio())
        .run()
        .await;
}

use neva::prelude::*;

#[tool]
async fn roots_request(mut ctx: Context) -> Result<String, Error> {
    let roots = ctx.list_roots().await?;
    Ok(format!("{:?}", roots.roots))
}

#[tokio::main]
async fn main() {
    App::new()
        .with_options(|opt| opt.with_default_http())
        .run()
        .await;
}

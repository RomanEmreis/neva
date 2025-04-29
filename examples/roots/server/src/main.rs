use neva::{App, Context, error::Error, tool};

#[tool]
async fn roots_request(mut ctx: Context) -> Result<String, Error> {
    let roots = ctx.list_roots().await?;
    Ok(format!("{:?}", roots.roots))
}

#[tokio::main]
async fn main() {
    let mut app = App::new()
        .with_options(|opt| opt
            .with_stdio()
            .with_mcp_version("2024-11-05"));
    
    map_roots_request(&mut app);
    
    app.run().await;
}

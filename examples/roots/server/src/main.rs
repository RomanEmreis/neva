use neva::{App, tool};

#[tool]
async fn roots_request() -> &'static str {
    "Roots requested!"
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

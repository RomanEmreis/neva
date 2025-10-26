//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector 
//!
//! cargo run -p large_resources_server
//! ```

use neva::prelude::*;

#[resource(uri = "meta://{name}")]
async fn resource_meta(name: String) -> ResourceContents {
    let uri: Uri = format!("file://{name}").into();
    let res = get_res_info(uri.clone(), name.clone());
    
    ResourceContents::new(uri)
        .with_title(name)
        .with_json(res)
}

#[resource(uri = "file://{name}")]
async fn resource_data(uri: Uri, name: String) -> ResourceContents {
    // get resource from somewhere
    
    ResourceContents::new(uri.clone())
        .with_title(name.clone())
        .with_blob("large file")
}

#[tool]
async fn get_file_info(ctx: Context, name: String) -> Result<Content, Error> {
    let res = ctx.resource(format!("meta://{name}")).await?;
    res.contents
        .into_iter()
        .next()
        .ok_or_else(|| Error::from(ErrorCode::ResourceNotFound))
        .and_then(|r| r.json::<Resource>())
        .map(|r| Content::link(r))
}

fn get_res_info(uri: Uri, name: String) -> Resource {
    Resource::new(uri, name)
        .with_size(100000)
        .with_mime("application/octet-stream")
        .with_descr("Large file")
}

#[tokio::main]
async fn main() {
    App::new()
        .with_options(|opt| opt.with_default_http())
        .run()
        .await;
}

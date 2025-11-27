//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector
//! 
//! cargo run -p example-pagination
//! ```

use std::sync::Arc;
use neva::prelude::*;

#[tool]
async fn validate_resource(ctx: Context, uri: Uri) -> Result<bool, Error> {
    let res = ctx.resource(uri).await?;
    Ok(!res.contents.is_empty())
}

#[resource(uri = "res://{name}")]
async fn get_resource(name: String, repo: Dc<ResourcesRepository>) -> (String, String) {
    repo.get_resource(name).await
}

#[resources]
async fn list_resources(params: ListResourcesRequestParams, repo: Dc<ResourcesRepository>) -> ListResourcesResult {
    repo.get_resources(params.cursor).await
}

#[completion]
async fn filter_resources(params: CompleteRequestParams, repo: Dc<ResourcesRepository>) -> Completion {
    let resources = &repo.resources;
    let filter = params.arg.value;
    
    let mut matched = Vec::new();
    let mut total = 0;

    for resource in resources.iter() {
        if !resource.uri.contains(&filter) {
            continue;
        }
        if total < 10 {
            matched.push(resource.uri.to_string());
        }
        total += 1;
    }

    Completion::new(matched, total)
}

#[tokio::main]
async fn main() {
    App::new()
        .with_options(|opt| opt
            .with_http(|http| http.bind("127.0.0.1:3000")))
        .add_singleton(ResourcesRepository::new())
        .run().await;
}

struct ResourcesRepository {
    resources: Arc<Vec<Resource>>,
}

impl ResourcesRepository {
    fn new() -> Self {
        let mut resources = vec![];
        for i in 0..10000 {
            resources.push(Resource::from(format!("res://test_{i}")));
        }
        Self { resources: Arc::new(resources) }
    }
    
    async fn get_resource(&self, name: String) -> (String, String) {
        (
            format!("res://{name}"),
            format!("Some details about resource: {name}")
        )
    }

    async fn get_resources(&self, cursor: Option<Cursor>) -> ListResourcesResult {
        self.resources.paginate(cursor, 10).into()
    }
}
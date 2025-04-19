//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector cargo run -p example-pagination
//! ```

use std::sync::Arc;
use neva::{App, resource};
use neva::types::{Resource, ListResourcesResult, Cursor, Pagination, Completion};

#[resource(uri = "res://{name}")]
async fn get_resource(name: String) -> (String, String) {
    (
        format!("res://{name}"),
        format!("Some details about resource: {name}")
    )
}

fn all_resources() -> Vec<Resource> {
    let mut resources = vec![];
    for i in 0..10000 {
        resources.push(Resource::from(format!("res://test_{i}")));
    }
    resources
}

async fn get_resources(resources: Arc<Vec<Resource>>, cursor: Option<Cursor>) -> ListResourcesResult {
    resources.paginate(cursor, 10).into()
}

async fn filter_resources(resources: Arc<Vec<Resource>>, filter: String) -> Completion {
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
    let mut app = App::new()
        .with_options(|opt| opt
            .with_stdio()
            .with_mcp_version("2024-11-05"));

    // Some global resource list
    let resources = Arc::new(all_resources());

    let res = Arc::clone(&resources);
    app.map_resources(move |params| get_resources(res.clone(), params.cursor));

    let res = Arc::clone(&resources);
    app.map_completion(move |params| filter_resources(res.clone(), params.arg.value));
    
    map_get_resource(&mut app);

    app.run().await;
}

//! MCP server resources

use neva::prelude::*;

// A synchronous listing: the catalogue is fixed, so there is nothing to await.
#[resources]
fn list_resources(_params: ListResourcesRequestParams) -> impl Into<ListResourcesResult> {
    [
        Resource::new("res://test1", "test 1")
            .with_descr("A test resource 1")
            .with_mime("text/plain"),
        Resource::new("res://test2", "test 2")
            .with_descr("A test resource 2")
            .with_mime("text/plain"),
    ]
}

#[resource(
    uri = "res://{name}",
    title = "Read resource",
    descr = "Some details about resource",
    mime = "text/plain",
    annotations = r#"{
        "audience": ["user"],
        "priority": 1.0
    }"#
)]
async fn get_res(name: String) -> TextResourceContents {
    TextResourceContents::new(
        format!("res://{name}"),
        format!("Some details about resource: {name}"),
    )
}

// A synchronous resource handler: no `async`, the contents are returned
// directly.
#[resource(uri = "txt://{name}", mime = "text/plain")]
fn get_text(name: String) -> TextResourceContents {
    TextResourceContents::new(format!("txt://{name}"), format!("Text for: {name}"))
}

// A blocking resource read: `std::fs` blocks, so it does not belong on a
// runtime worker.
#[resource(uri = "file://{name}", mime = "text/plain", blocking)]
fn read_file_resource(name: String) -> TextResourceContents {
    let text = std::fs::read_to_string(&name).unwrap_or_default();
    TextResourceContents::new(format!("file://{name}"), text)
}

#[resource(uri = "res://err/{uri}")]
async fn err_resource(_uri: Uri) -> Result<ResourceContents, Error> {
    #[allow(deprecated)]
    Err(Error::from(ErrorCode::ResourceNotFound))
}

// A synchronous completion handler: it filters an in-memory list.
#[completion]
fn complete_resource(params: CompleteRequestParams) -> Completion {
    let matched = ["res://test1", "res://test2"]
        .into_iter()
        .filter(|uri| uri.contains(&params.arg.value))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let total = matched.len();

    Completion::new(matched, total)
}

//! MCP server resources

use neva::prelude::*;

#[resources]
async fn list_resources(_params: ListResourcesRequestParams) -> impl Into<ListResourcesResult> {
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
        format!("Some details about resource: {name}"))
}

#[resource(uri = "res://err/{uri}")]
async fn err_resource(_uri: Uri) -> Result<ResourceContents, Error> {
    Err(Error::from(ErrorCode::ResourceNotFound))
}
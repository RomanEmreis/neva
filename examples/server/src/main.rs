//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector cargo run -p example-server
//! ```

use neva::{
    App, 
    tool, resource, prompt, 
    types::{Json, Resource, ListResourcesRequestParams, ListResourcesResult}
};

#[derive(serde::Deserialize)]
struct Payload {
    say: String,
    name: String,
}

#[derive(serde::Serialize)]
struct Result {
    message: String,
}

#[tool(descr = "Hello world tool")]
async fn say_hello() -> &'static str {
    "Hello, world!"
}

#[tool(
    descr = "Hello to name tool",
    schema = r#"{
        "properties": {
            "name": { "type": "string", "description": "A name to whom say Hello" }
        }
    }"#
)]
async fn say_hello_to(name: String) -> String {
    format!("Hello, {name}!")
}

#[tool(descr = "Say from JSON")]
async fn say_json(arg: Json<Payload>) -> Json<Result> {
    let result = Result { message: format!("{}, {}!", arg.say, arg.name) };
    result.into()
}

#[resource(
    uri = "res://{name}",
    descr = "Some details about resource",
    mime = "text/plain",
    annotations = r#"{
        "audience": ["user"],
        "priority": 1.0
    }"#
)]
async fn get_res(name: String) -> [(String, String); 1] {
    let content = (
        format!("res://{name}"),
        format!("Some details about resource: {name}")
    );
    [content]
}

#[prompt(
    descr = "Analyze code for potential improvements",
    args = r#"[
        { 
            "name": "lang", 
            "description": "A language to use", 
            "required": true
        }    
    ]"#
)]
async fn analyze_code(lang: String) -> [(String, String); 1] {
    [
        (format!("Language: {lang}"), "user".into())
    ]
}

async fn list_resources(_params: ListResourcesRequestParams) -> impl Into<ListResourcesResult> {
    [
        Resource::new("res://test1", "test 1")
            .with_description("A test resource 1")
            .with_mime("text/plain"),
        Resource::new("res://test2", "test 2")
            .with_description("A test resource 2")
            .with_mime("text/plain"),
    ]
}

#[tokio::main]
async fn main() {
    let mut app = App::new()
        .with_options(|opt| opt
            .with_stdio()
            .with_name("sample mcp server")
            .with_version("0.1.0.0"));

    map_say_hello(&mut app);
    map_say_hello_to(&mut app);
    map_say_json(&mut app);

    map_get_res(&mut app);

    map_analyze_code(&mut app);

    app.map_resources(list_resources);

    app.run().await;
}

//! Run with:
//!
//! ```no_rust
//! npx @modelcontextprotocol/inspector cargo run -p example-server
//! ```

use neva::prelude::*;

#[derive(serde::Deserialize)]
struct Payload {
    say: String,
    name: String,
}

#[json_schema(ser)]
struct Results {
    message: String,
}

#[tool(descr = "Hello world tool")]
async fn say_hello() -> &'static str {
    "Hello, world!"
}

#[tool(
    descr = "Hello to name tool",
    input_schema = r#"{
        "properties": {
            "name": { "type": "string", "description": "A name to whom say Hello" }
        }
    }"#
)]
async fn say_hello_to(name: String) -> String {
    format!("Hello, {name}!")
}

#[tool(
    descr = "Say from JSON",
    output_schema = r#"{
        "properties": {
            "message": { "type": "string", "description": "A message to say" }
        },
        "required": ["message"]
    }"#
)]
async fn say_json(arg: Json<Payload>) -> Json<Results> {
    let result = Results { message: format!("{}, {}!", arg.say, arg.name) };
    result.into()
}

#[tool(
    title = "JSON Hello",
    descr = "Say from JSON",
    input_schema = r#"{
        "properties": {
            "say": { "type": "string", "description": "A message to say" },
            "name": { "type": "string", "description": "A name to whom say Hello" }
        },
        "required": ["say", "name"]
    }"#
)]
async fn say_out_json(say: String, name: String) -> Json<Results> {
    let result = Results { message: format!("{say}, {name}!") };
    result.into()
}

#[tool(
    descr = "A tool with error",
    annotations = r#"{
        "title": "Error Tool",
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false,
        "readOnlyHint": false
    }"#
)]
async fn tool_error() -> Result<String, Error> {
    Err(Error::from(ErrorCode::InternalError))
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

#[prompt(
    title = "Code Analyzer",
    descr = "Analyze code for potential improvements",
    args = r#"[
        {
            "name": "lang", 
            "description": "A language to use", 
            "required": true
        }    
    ]"#
)]
async fn analyze_code(lang: String) -> PromptMessage {
    PromptMessage::user()
        .with(format!("Language: {lang}"))
}

#[prompt(descr = "A prompt that return error")]
async fn prompt_err() -> Result<PromptMessage, Error> {
    Err(Error::from(ErrorCode::InvalidRequest))
}

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

#[handler(command = "ping")]
async fn ping_handler() {
    eprintln!("pong");
}

#[tokio::main]
async fn main() {
    App::new()
        .with_options(|opt| opt
            .with_stdio()
            .with_mcp_version("2025-06-18")
            .with_name("Sample MCP Server")
            .with_version("0.1.0.0")
            .with_tools(|tools| tools
                .with_list_changed()))
        .run()
        .await;
}

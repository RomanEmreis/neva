//! MCP server tools

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
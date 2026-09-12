//! MCP server tools

use neva::prelude::*;

#[derive(serde::Deserialize, serde::Serialize)]
struct Payload {
    say: String,
    name: String,
}

#[json_schema(serde)]
struct Results {
    message: String,
}

#[tool(descr = "Hello world tool")]
async fn say_hello() -> &'static str {
    "Hello, world!"
}

// A synchronous handler: no `async`, the value is returned directly. The body
// runs on the runtime thread that dispatched the call, which is right for pure
// computation -- see `read_file` below for what to do when the body blocks.
#[tool(descr = "Sums two numbers")]
fn sum(a: i32, b: i32) -> i32 {
    a + b
}

// A blocking handler: `blocking` moves the body onto Tokio's blocking pool, so
// the file read does not stall the runtime worker that dispatched the call.
#[tool(descr = "Reads a text file", blocking)]
fn read_file(path: String) -> Result<String, Error> {
    std::fs::read_to_string(path).map_err(|err| Error::new(ErrorCode::InternalError, err))
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
    input_schema = r#"{
        "properties": {
            "arg": { 
                "type": "object", 
                "description": "A message in JSON format", 
                "properties": {
                    "say": { "type": "string", "description": "A message to say" },
                    "name": { "type": "string", "description": "A name to whom say Hello" }
                },
                "required": ["say", "name"] 
            }
        },
        "required": ["arg"]
    }"#,
    output_schema = r#"{
        "properties": {
            "message": { "type": "string", "description": "A message to say" }
        },
        "required": ["message"]
    }"#
)]
async fn say_json(arg: Json<Payload>) -> Json<Results> {
    let result = Results {
        message: format!("{}, {}!", arg.say, arg.name),
    };
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
    let result = Results {
        message: format!("{say}, {name}!"),
    };
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

#[tool(descr = "Resource metadata")]
async fn read_resource(ctx: Context, res: Uri) -> Result<Content, Error> {
    let result = ctx.resource(res).await?;
    let resource = result
        .contents
        .into_iter()
        .next()
        .expect("No resource contents");
    Ok(Content::resource(resource))
}

//! Run with:
//! 
//! ```no_rust
//! npx @modelcontextprotocol/inspector cargo run --example server
//! ```

use neva::{App, types::{Json, Resource, ResourceContents}};
use neva::error::Error;
use neva::types::{content, Role, Uri};

#[tokio::main]
async fn main() {
    let mut app = App::new()
        .with_options(|opt| opt
            .with_stdio()
            .with_name("sample mcp server")
            .with_version("0.1.0.0"));

    app.map_tool("say_hello", || async {
        "Hello, world!"
    });
    
    app.map_tool("say_hello_to", |name: String| async move {
       format!("Hello, {name}!")
    });

    app.map_tool("say_json", |arg: Json<Payload>| async move {
        let result = Result { message: format!("{}, {}!", arg.say, arg.name) };
        Json::from(result)
    });

    app.map_tool("v2/say_json", |arg: serde_json::Value| async move {
        arg
    });
    
    app.map_tool("test_error", |throw: bool| async move {
        if throw {
            Err(Error::new("some error"))
        } else {
            Ok("no error")
        }
    });
    
    app.map_resources(|_params| async move {
        [
            Resource::new("res://test1", "test 1"),
            Resource::new("res://test2", "test 2")
        ]
    });
    
    app.map_resource("res://{name}", "get_res", |name: String| async move {
        let content = (
            format!("res://{name}"),
            format!("Some details about resource: {name}")
        );
        [content]
    });
    
    app.map_prompt("analyze-code", |lang: String| async move {
        [
            (format!("Language: {lang}"), Role::User)
        ]
    });
    
    app.run().await;
}

#[derive(serde::Deserialize)]
struct Payload {
    say: String,
    name: String,
}

#[derive(serde::Serialize)]
struct Result {
    message: String,
}
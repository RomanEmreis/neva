//! Run with:
//! 
//! ```no_rust
//! npx @modelcontextprotocol/inspector cargo run --example server
//! ```

use neva::{App, types::{Json, Resource}};
use neva::error::Error;

#[tokio::main]
async fn main() {
    let mut app = App::new()
        .with_options(|opt| opt
            .with_stdio()
            .with_name("sample mcp server")
            .with_version("0.1.0.0"));

    app.map_tool("say_hello", || async { "Hello, world!" })
        .with_description("Hello world tool");
    
    app.map_tool("say_hello_to", |name: String| async move { format!("Hello, {name}!") })
        .with_description("Hello to name tool")
        .with_schema(|schema| schema
            .add_property(
                "name", 
                "A name to whom say hello", 
                "string")
        );

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
            Resource::new("res://test1", "test 1")
                .with_description("A test resource 1")
                .with_mime("text/plain"),
            Resource::new("res://test2", "test 2")
                .with_description("A test resource 2")
                .with_mime("text/plain"),
        ]
    });
    
    app.map_resource("res://{name}", "get_res", |name: String| async move {
        let content = (
            format!("res://{name}"),
            format!("Some details about resource: {name}")
        );
        [content]
    })
        .with_description("Some details about resource")
        .with_mime("text/plain")
        .with_annotations(|annotations| annotations
            .set_priority(1.0)
            .add_audience("user"));
    
    app.map_prompt("analyze-code", |lang: String| async move {
        [
            (format!("Language: {lang}"), "user")
        ]
    })
        .with_description("Analyze code for potential improvements")
        .with_args(["lang"]);
    
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
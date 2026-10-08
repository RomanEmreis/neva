//! A remote MCP server's tools and prompt, handed to a model.
//!
//! ```sh
//! # terminal 1: the task board, served over MCP
//! cargo run -p server
//!
//! # terminal 2
//! SVIR_MODEL=<model> cargo run -p client
//! ```
//!
//! `SVIR_URL` names the model server, `http://127.0.0.1:1234` (LM Studio)
//! unless set.

use neva::{Client, svir::RemoteTools};
use svir::Toolbox;

/// A model that keeps calling tools is stopped after this many answers.
const TURNS: usize = 8;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = Client::new().with_options(|opt| {
        opt.with_http(|http| http.bind("127.0.0.1:3000").with_endpoint("/mcp"))
    });
    client.connect().await?;

    // The server's tools, offered as `board_add_task` and so on: a prefix
    // keeps them apart from any other server's in the same request.
    let tools = RemoteTools::new(client).with_prefix("board_").load().await?;

    // The conversation starts from the server's own prompt.
    let prompt = tools
        .client()
        .prompts()
        .get("plan_week", [("focus", "the release")])
        .await?;

    let request = neva::svir::prompt_messages(prompt)?.into_iter().fold(
        svir::Request::new(model_name())
            .system("You keep the user's task board with the tools you have.")
            .tools(&tools),
        svir::Request::message,
    );

    let model = svir::Client::openai(url()).build()?;
    println!("{}", converse(&model, request, &tools).await?);
    Ok(())
}

/// Answers the model's tool calls until it answers the user.
async fn converse(
    model: &svir::Client,
    mut request: svir::Request,
    tools: &(impl Toolbox + Sync),
) -> Result<String, Box<dyn std::error::Error>> {
    for _ in 0..TURNS {
        let done = model.complete(&request).await?;
        if done.calls.is_empty() {
            return Ok(done.text.trim().to_string());
        }

        for call in &done.calls {
            eprintln!("-- {}({})", call.name, call.arguments);
        }
        // Over the shared client, the calls of one turn run concurrently.
        let results = tools.call_all(&done.calls).await;
        request = request.assistant(done).tool_results(results);
    }

    Err("the model kept calling tools".into())
}

/// The model server.
fn url() -> String {
    std::env::var("SVIR_URL").unwrap_or_else(|_| "http://127.0.0.1:1234".to_owned())
}

/// The model.
fn model_name() -> String {
    std::env::var("SVIR_MODEL").unwrap_or_else(|_| {
        eprintln!("set SVIR_MODEL to a model the server at {} has", url());
        std::process::exit(2)
    })
}

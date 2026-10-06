//! A task board, written once with `#[tool]` and `#[prompt]`.
//!
//! ```sh
//! # served over MCP, for `cargo run -p client`
//! cargo run -p server
//!
//! # or handed to a model in this process, with no MCP in between
//! SVIR_MODEL=<model> cargo run -p server -- --local
//! ```
//!
//! `SVIR_URL` names the model server, `http://127.0.0.1:1234` (LM Studio)
//! unless set.

use neva::{di::Dc, error::Error, prelude::*};
use std::sync::Mutex;
use svir::Toolbox;

/// A model that keeps calling tools is stopped after this many answers.
const TURNS: usize = 8;

/// The tasks, shared by every tool through dependency injection.
#[derive(Default)]
struct Board {
    tasks: Mutex<Vec<(String, bool)>>,
}

impl Board {
    fn tasks(&self) -> std::sync::MutexGuard<'_, Vec<(String, bool)>> {
        self.tasks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[tool(descr = "Adds a task to the board and returns its number.")]
async fn add_task(board: Dc<Board>, title: String) -> String {
    let mut tasks = board.tasks();
    tasks.push((title, false));
    format!("added as task {}", tasks.len())
}

#[tool(descr = "Marks the task with this number as done.")]
async fn complete_task(board: Dc<Board>, number: usize) -> Result<String, Error> {
    let mut tasks = board.tasks();
    let task = number
        .checked_sub(1)
        .and_then(|index| tasks.get_mut(index))
        .ok_or_else(|| Error::new(ErrorCode::InvalidParams, format!("no task {number}")))?;
    task.1 = true;
    Ok(format!("task {number} is done"))
}

#[tool(descr = "Lists every task on the board, done or not.")]
async fn list_tasks(board: Dc<Board>) -> String {
    let tasks = board.tasks();
    if tasks.is_empty() {
        return "the board is empty".to_string();
    }
    tasks
        .iter()
        .enumerate()
        .map(|(index, (title, done))| {
            let mark = if *done { "x" } else { " " };
            format!("{}. [{mark}] {title}", index + 1)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[prompt(descr = "Plans the week around one focus, keeping the board up to date.")]
async fn plan_week(focus: String) -> PromptMessage {
    PromptMessage::user().with(format!(
        "Plan my week around {focus}. Put three tasks on the board, mark the \
         first one done since I finished it this morning, then show me the board."
    ))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = App::new()
        .with_options(|opt| {
            opt.with_name("Task board")
                .with_http(|http| http.bind("127.0.0.1:3000").with_endpoint("/mcp"))
        })
        .add_singleton(Board::default());

    if !std::env::args().any(|arg| arg == "--local") {
        app.run().await;
        return Ok(());
    }

    // The same functions, called in this process: through the server's
    // pipeline, with `Dc<Board>` injected as over MCP.
    let tools = app.into_toolbox().load().await?;
    let model = svir::Client::openai(url()).build()?;
    let request = svir::Request::new(model_name())
        .system("You keep the user's task board with the tools you have.")
        .tools(&tools)
        .user(
            "Plan my week around the release. Put three tasks on the board, mark the \
             first one done, then show me the board.",
        );

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

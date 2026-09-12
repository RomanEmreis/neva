//! MCP server prompts

use neva::prelude::*;

#[prompt(
    title = "Hello World generator",
    descr = "Generates a user message requesting a hello world code generation.",
    args = r#"[
        {
            "name": "lang", 
            "description": "A language to use", 
            "required": true
        }    
    ]"#
)]
async fn hello_world_code(lang: String) -> PromptMessage {
    PromptMessage::user().with(format!("Write a hello-world function on {lang}"))
}

// A synchronous prompt handler: no `async`, the message is returned directly.
#[prompt(descr = "Asks for a code review.")]
fn review_code(lang: String) -> PromptMessage {
    PromptMessage::user().with(format!("Review this {lang} code"))
}

// A blocking prompt handler: the template is read from disk, so the body goes
// to the blocking pool instead of the runtime worker.
#[prompt(descr = "Loads a prompt template from disk.", blocking)]
fn load_template(name: String) -> PromptMessage {
    let body = std::fs::read_to_string(format!("templates/{name}.txt")).unwrap_or_default();
    PromptMessage::user().with(body)
}

#[prompt(descr = "A prompt that return error")]
async fn prompt_err() -> Result<PromptMessage, Error> {
    Err(Error::from(ErrorCode::InvalidRequest))
}

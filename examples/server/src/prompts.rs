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
    PromptMessage::user()
        .with(format!("Write a hello-world function on {lang}"))
}

#[prompt(descr = "A prompt that return error")]
async fn prompt_err() -> Result<PromptMessage, Error> {
    Err(Error::from(ErrorCode::InvalidRequest))
}
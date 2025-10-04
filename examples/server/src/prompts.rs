//! MCP server prompts

use neva::prelude::*;

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
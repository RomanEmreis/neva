//! Run with:
//!
//! ```no_rust
//! cargo run -p example-client
//! ```

use std::time::Duration;
use neva::Client;
use neva::error::Error;
use tracing_subscriber::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();
    
    let mut client = Client::new()
        .with_options(|opt| opt
            .with_stdio("npx", ["-y", "@modelcontextprotocol/server-everything"])
            .with_roots(|roots| roots.with_list_changed())
            .with_timeout(Duration::from_secs(5))
            .with_mcp_version("2024-11-05"));
    
    client.connect().await?;
    
    // List tools
    tracing::info!("--- LIST TOOLS ---");
    let tools = client.list_tools(None).await?;
    for tool in tools.tools {
        tracing::info!("- {}", tool.name);
    }
    
    // Call a tool
    let args = ("message", "Hello MCP!");
    tracing::info!("--- CALL TOOL ---");
    let result = client.call_tool("echo", args).await?;
    tracing::info!("{:?}", result.content);
    
    // List resources
    tracing::info!("--- LIST RESOURCES ---");
    let resources = client.list_resources(None).await?;

    tracing::info!("--- PAGE: 1 ---");
    for res in resources.resources {
        tracing::info!("- {}: {:?}", res.name, res.uri);
    }
    // Fetch the next "page"
    let resources = client.list_resources(resources.next_cursor).await?;
    tracing::info!("--- PAGE: 2 ---");
    for res in resources.resources {
        tracing::info!("- {}: {:?}", res.name, res.uri);
    }
    
    // List templates
    tracing::info!("--- LIST RESOURCE TEMPLATES ---");
    let templates = client.list_resource_templates(None).await?;
    for template in templates.templates {
        tracing::info!("- {}: {:?}", template.name, template.uri_template);
    }

    // Read resource
    tracing::info!("--- READ RESOURCE ---");
    let resource = client.read_resource("test://static/resource/1").await?;
    tracing::info!("{:?}", resource.contents);
    
    // List prompts
    tracing::info!("--- LIST PROMPTS ---");
    let prompts = client.list_prompts(None).await?;
    for prompt in prompts.prompts {
        tracing::info!("- {}", prompt.name);
    }
    
    // Get prompt
    tracing::info!("--- GET PROMPT ---");
    let args = [
        ("temperature", "50"),
        ("style", "anything")
    ];
    let prompt = client.get_prompt("complex_prompt", args).await?;
    tracing::info!("{:?}: {:?}", prompt.descr, prompt.messages);
    
    // This can be uncommented to check the log notifications from MCP server
    //tokio::time::sleep(Duration::from_secs(60)).await;
    
    client.disconnect().await
}

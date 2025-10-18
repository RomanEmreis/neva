//! Run with:
//!
//! ```no_rust
//! cargo run -p example-client
//! ```

use std::time::Duration;
use neva::prelude::*;
use tracing_subscriber::prelude::*;

#[allow(dead_code)]
#[json_schema(de, debug)]
struct Weather {
    conditions: String,
    temperature: f32,
    humidity: f32,
}

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
    
    // Ping command
    tracing::info!("--- PING ---");
    let resp = client.ping().await?;
    tracing::info!("{:?}", resp);
    
    // List tools
    tracing::info!("--- LIST TOOLS ---");
    let tools = client.list_tools(None).await?;
    for tool in tools.tools.iter() {
        tracing::info!("- {}", tool.name);
    }
    
    // Call a tool
    tracing::info!("--- CALL TOOL ---");
    let args = ("message", "Hello MCP!");
    let result = client.call_tool("echo", args).await?;
    tracing::info!("{:?}", result.content);

    // Structured content
    tracing::info!("--- STRUCTURED CONTENT ---");
    let tool = tools.get("structuredContent").unwrap();
    let args = ("location", "London");
    let result = client.call_tool(&tool.name, args).await?;
    let weather: Weather = tool
        .validate(&result)
        .and_then(|res| res.as_json())?;
    tracing::info!("{:?}", weather);
    
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

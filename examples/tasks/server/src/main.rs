use neva::prelude::*;

#[tool(task_support = "required")]
async fn endless_tool() -> &'static str {
    // Simulate a long-running task
    //loop {
    //    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    //}
    
    "Completed successfully!"
}

fn main() {
    App::new()
        .with_options(|opt| opt
            .with_stdio()
            .with_tasks(|t| t.with_all()))
        .run_blocking();
}
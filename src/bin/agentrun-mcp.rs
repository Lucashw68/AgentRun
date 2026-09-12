use agentrun::{core::AgentRun, mcp::McpServer};
use rmcp::ServiceExt;

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args_os().len() != 1 {
        return Err("Usage: agentrun-mcp (stdio only; no options)".into());
    }
    let server = McpServer::new(AgentRun::from_environment()?);
    let service = server.serve(agentrun::mcp::bounded_stdio()).await?;
    service.waiting().await?;
    Ok(())
}

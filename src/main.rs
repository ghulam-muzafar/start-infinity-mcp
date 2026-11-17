mod infinity;
mod models;
mod tools;

use infinity::InfinityClient;
use rmcp::{ServiceExt, transport::stdio};
use tools::InfinityServer;
use tracing_subscriber::EnvFilter;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let token = std::env::var("INFINITY_API_TOKEN")
        .map_err(|_| "Set INFINITY_API_TOKEN in the MCP client's environment before starting")?;
    let client = InfinityClient::new(&token)?;
    let service = InfinityServer::new(client).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

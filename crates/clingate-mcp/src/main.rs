//! `clingate-mcp`: clingate's tools, served to Claude Desktop (or any MCP
//! client) over stdin and stdout.

use clingate_mcp::Clingate;
use rmcp::ServiceExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Diagnostics to stderr: stdout is the protocol, and anything else
    // written there would corrupt it.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("CLINGATE_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let service = Clingate::new().serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}

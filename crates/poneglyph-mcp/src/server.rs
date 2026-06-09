//! MCP stdio server bootstrap.
//!
//! stdout carries JSON-RPC — all logging must go to stderr (the CLI sets
//! that up before calling in here).

use anyhow::{Context, Result};
use rmcp::ServiceExt;
use tracing::info;

use crate::tools::PoneglyphMcp;

/// Serve MCP over stdio until the client disconnects or the process is killed.
pub async fn run_stdio(server: PoneglyphMcp) -> Result<()> {
    info!("MCP stdio server starting");
    let service = server
        .serve(rmcp::transport::stdio())
        .await
        .context("MCP server failed to start")?;
    service.waiting().await.context("MCP server terminated abnormally")?;
    info!("MCP stdio server stopped");
    Ok(())
}

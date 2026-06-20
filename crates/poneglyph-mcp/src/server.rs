//! MCP server bootstrap — Streamable HTTP (default) and stdio (`--stdio`).
//!
//! stdio mode: stdout carries JSON-RPC — all logging must go to stderr (the
//! CLI sets that up before calling in here).

use std::sync::Arc;

use anyhow::{Context, Result};
use rmcp::ServiceExt;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
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

/// Build a tower service that serves MCP over Streamable HTTP — mount it
/// into an axum router, e.g. `.nest_service("/mcp", streamable_http_service(mcp))`.
///
/// `PoneglyphMcp` is cheap to clone (every field is an `Arc`), so the
/// per-session factory just clones the one shared handle rather than opening
/// a new store/embedder per connection.
pub fn streamable_http_service(
    server: PoneglyphMcp,
) -> StreamableHttpService<PoneglyphMcp, LocalSessionManager> {
    StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    )
}

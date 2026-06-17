//! poneglyph HTTP server (PRD §8.7–8.8, §12).
//!
//! Two jobs: serve the viewer API (`/api/*`) and receive passive-capture
//! events (`/ingest`). All business logic lives in `poneglyph-core`.

pub mod api;
pub mod auth;
pub mod error;
pub mod ingest;
pub mod state;
pub mod viewer;

use std::net::IpAddr;

use anyhow::{Context, Result};
use axum::middleware;
use axum::routing::get;
use axum::Router;
use tower_http::trace::TraceLayer;

use poneglyph_core::config::Config;

pub use state::AppState;

/// Refuse to start when the bind address is non-loopback and no API token is
/// configured (PRD §12). Call before binding anything.
pub fn validate_security(config: &Config) -> Result<()> {
    let addr: IpAddr = config
        .dashboard
        .host
        .parse()
        .with_context(|| format!("dashboard.host is not a valid IP address: {}", config.dashboard.host))?;
    let token_ok = config
        .dashboard
        .token
        .as_deref()
        .is_some_and(|t| !t.trim().is_empty());
    if !addr.is_loopback() && !token_ok {
        anyhow::bail!(
            "refusing to start: dashboard.host {addr} is non-loopback and dashboard.token is unset"
        );
    }
    Ok(())
}

pub fn build_router(state: AppState) -> Router {
    let api_routes = Router::new()
        .route("/memories", get(api::list_memories))
        .route(
            "/memories/{id}",
            get(api::get_memory).patch(api::patch_memory).delete(api::delete_memory),
        )
        .route("/search", get(api::search))
        .route("/graph", get(api::graph))
        .route("/context", get(api::project_context))
        .route("/timeline", get(api::timeline))
        .route("/projects", get(api::list_projects))
        .route("/stats", get(api::stats))
        .route("/settings", get(api::get_settings).patch(api::patch_settings));

    let guarded = Router::new()
        .nest("/api", api_routes)
        .route("/ingest", axum::routing::post(ingest::ingest))
        .layer(middleware::from_fn_with_state(state.clone(), auth::require_token));

    Router::new()
        .merge(guarded)
        .route("/healthz", get(api::healthz))
        .fallback(viewer::static_handler)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Bind the configured address. Split from [`serve_on`] so the caller can
/// decide what a bind failure means (e.g. degrade to MCP-only on AddrInUse).
pub async fn bind(config: &Config) -> std::io::Result<tokio::net::TcpListener> {
    let addr = format!("{}:{}", config.dashboard.host, config.dashboard.port);
    tokio::net::TcpListener::bind(&addr).await
}

/// Run the HTTP server on an already-bound listener until process end.
pub async fn serve_on(listener: tokio::net::TcpListener, state: AppState) -> Result<()> {
    let addr = listener.local_addr()?;
    tracing::info!(%addr, "HTTP server listening");
    axum::serve(listener, build_router(state))
        .await
        .context("HTTP server failed")
}

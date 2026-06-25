//! Shared state for the HTTP server.
//!
//! Mirrors the `PoneglyphMcp` pattern: the store sits behind a sync mutex
//! shared with the MCP server, embedding runs *before* the lock is taken,
//! and nothing awaits while holding it.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};

use poneglyph_core::config::Config;
use poneglyph_core::embed::Embedder;
use poneglyph_core::enrich::EnrichHandle;
use poneglyph_core::store::Store;

use crate::error::ApiError;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Mutex<Store>>,
    /// None ⇒ degrade gracefully: FTS-only search, no vec indexing.
    pub embedder: Option<Arc<Embedder>>,
    pub config: Arc<Config>,
    /// Wake-up handle for the background edge worker.
    pub enrich: Option<EnrichHandle>,
    /// Project IDs with a pending graph rebuild (shared with the MCP
    /// server's file-watch supervisor). `None` when no watcher is running
    /// in this process (e.g. the standalone `viewer` command).
    pub graph_dirty: Option<Arc<Mutex<HashSet<String>>>>,
    /// Live-status registry shared with the enrich worker + graph supervisor.
    /// `None` ⇒ `/api/activity` reports no in-flight phases (still serves job
    /// queue + dirty-project counts).
    pub activity: Option<Arc<poneglyph_core::activity::Activity>>,
}

impl AppState {
    /// Mirrors `PoneglyphMcp::is_graph_dirty` — true if a file change for
    /// this project is still awaiting a debounced graph rebuild.
    pub fn is_graph_dirty(&self, project_id: &str) -> bool {
        self.graph_dirty.as_ref().is_some_and(|d| d.lock().is_ok_and(|d| d.contains(project_id)))
    }
}

impl AppState {
    pub fn lock_store(&self) -> Result<MutexGuard<'_, Store>, ApiError> {
        self.store.lock().map_err(|_| ApiError::internal("store mutex poisoned"))
    }

    /// Embed text being stored (memory content/edits). `None` when no
    /// embedder is configured — callers fall back to FTS-only indexing.
    pub async fn embed_passage_or_none(&self, text: &str) -> Result<Option<Vec<f32>>, ApiError> {
        match &self.embedder {
            Some(e) => e.embed_passage(text).await.map(Some).map_err(ApiError::internal),
            None => Ok(None),
        }
    }

    /// Embed a search query. `None` when no embedder is configured —
    /// callers fall back to FTS-only search.
    pub async fn embed_query_or_none(&self, text: &str) -> Result<Option<Vec<f32>>, ApiError> {
        match &self.embedder {
            Some(e) => e.embed_query(text).await.map(Some).map_err(ApiError::internal),
            None => Ok(None),
        }
    }
}

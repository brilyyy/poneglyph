//! Shared state for the HTTP server.
//!
//! Mirrors the `PoneglyphMcp` pattern: the store sits behind a sync mutex
//! shared with the MCP server, embedding runs *before* the lock is taken,
//! and nothing awaits while holding it.

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
}

impl AppState {
    pub fn lock_store(&self) -> Result<MutexGuard<'_, Store>, ApiError> {
        self.store.lock().map_err(|_| ApiError::internal("store mutex poisoned"))
    }

    pub async fn embed_or_none(&self, text: &str) -> Result<Option<Vec<f32>>, ApiError> {
        match &self.embedder {
            Some(e) => e
                .embed_text(text)
                .await
                .map(Some)
                .map_err(|e| ApiError::internal(e)),
            None => Ok(None),
        }
    }
}

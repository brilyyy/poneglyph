use anyhow::{Context, Result};
use embed_anything::embeddings::embed::{Embedder as EaEmbedder, TextEmbedder};
use std::sync::Arc;
use tracing::info;

use crate::config::Config;

const DEFAULT_MODEL_ID: &str = "BAAI/bge-small-en-v1.5";
const DEFAULT_BATCH_SIZE: usize = 32;

pub struct Embedder {
    inner: Arc<TextEmbedder>,
    dimensions: usize,
}

impl Embedder {
    pub async fn new(config: &Config) -> Result<Self> {
        let model_id = if config.embedding.model_id.is_empty() {
            DEFAULT_MODEL_ID.to_string()
        } else {
            config.embedding.model_id.clone()
        };
        let dimensions = config.embedding.dimensions;

        info!(model = %model_id, "loading embedding model");

        let ea = tokio::task::spawn_blocking(move || {
            EaEmbedder::from_pretrained_hf(&model_id, None, None, None)
        })
        .await
        .context("embedder task panicked")?
        .context("failed to load embedding model")?;

        let inner: TextEmbedder = ea.into();
        info!("embedding model loaded");

        Ok(Self {
            inner: Arc::new(inner),
            dimensions,
        })
    }

    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    pub async fn embed_text(&self, text: &str) -> Result<Vec<f32>> {
        let results = self.embed_batch(&[text]).await?;
        results.into_iter().next().context("empty embedding result")
    }

    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let embedder = Arc::clone(&self.inner);
        let results = embedder
            .embed(texts, Some(DEFAULT_BATCH_SIZE), None)
            .await
            .context("embedding failed")?;

        results
            .into_iter()
            .map(|r| r.to_dense().context("converting to dense"))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let cfg = Config::default();
        assert_eq!(cfg.embedding.model_id, "BAAI/bge-small-en-v1.5");
        assert_eq!(cfg.embedding.dimensions, 384);
    }

    #[tokio::test]
    #[ignore] // requires model download on first run
    async fn embed_single_text() {
        let cfg = Config::default();
        let embedder = Embedder::new(&cfg).await.unwrap();
        let vec = embedder.embed_text("hello world").await.unwrap();
        assert_eq!(vec.len(), 384);
        assert!(vec.iter().all(|v| v.is_finite()));
    }

    #[tokio::test]
    async fn embed_batch_texts() {
        let cfg = Config::default();
        let embedder = Embedder::new(&cfg).await.unwrap();
        let texts = vec!["hello", "world", "test"];
        let results = embedder.embed_batch(&texts).await.unwrap();
        assert_eq!(results.len(), 3);
        for vec in &results {
            assert_eq!(vec.len(), 384);
            assert!(vec.iter().all(|v| v.is_finite()));
        }
    }
}

use anyhow::{Context, Result};
use embed_anything::embeddings::embed::{Embedder as EaEmbedder, TextEmbedder};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tracing::info;

use crate::config::Config;

const DEFAULT_MODEL_ID: &str = "BAAI/bge-small-en-v1.5";
const DEFAULT_BATCH_SIZE: usize = 32;

const OLLAMA_DEFAULT_BASE_URL: &str = "http://localhost:11434";
const OPENAI_DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const OPENAI_DEFAULT_EMBED_MODEL: &str = "text-embedding-3-small";

enum Backend {
    /// Candle-backed local inference (embed_anything) — default, offline.
    Local(Arc<TextEmbedder>),
    /// Ollama `/api/embeddings` — one request per text (no native batch endpoint).
    Ollama { http: reqwest::Client, base_url: String, model: String },
    /// OpenAI-compatible `/embeddings` — natively batched.
    OpenAi { http: reqwest::Client, base_url: String, api_key: String, model: String },
}

pub struct Embedder {
    backend: Backend,
    dimensions: usize,
}

impl Embedder {
    pub async fn new(config: &Config) -> Result<Self> {
        let dimensions = config.embedding.dimensions;
        let model_id = if config.embedding.model_id.is_empty() {
            DEFAULT_MODEL_ID.to_string()
        } else {
            config.embedding.model_id.clone()
        };

        let backend = match config.embedding.provider.as_str() {
            "ollama" => Backend::Ollama {
                http: reqwest::Client::new(),
                base_url: non_empty(config.llm.base_url.as_deref()).unwrap_or(OLLAMA_DEFAULT_BASE_URL).to_string(),
                model: model_id,
            },
            "openai" => Backend::OpenAi {
                http: reqwest::Client::new(),
                base_url: OPENAI_DEFAULT_BASE_URL.to_string(),
                api_key: config.llm.api_key.clone().unwrap_or_default(),
                model: non_empty(Some(&model_id)).unwrap_or(OPENAI_DEFAULT_EMBED_MODEL).to_string(),
            },
            _ => {
                info!(model = %model_id, "loading embedding model");
                let ea = tokio::task::spawn_blocking(move || {
                    EaEmbedder::from_pretrained_hf(&model_id, None, None, None)
                })
                .await
                .context("embedder task panicked")?
                .context("failed to load embedding model")?;
                let inner: TextEmbedder = ea.into();
                info!("embedding model loaded");
                Backend::Local(Arc::new(inner))
            }
        };

        Ok(Self { backend, dimensions })
    }

    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    pub async fn embed_text(&self, text: &str) -> Result<Vec<f32>> {
        let results = self.embed_batch(&[text]).await?;
        results.into_iter().next().context("empty embedding result")
    }

    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let vectors = match &self.backend {
            Backend::Local(embedder) => embed_local(embedder, texts).await?,
            Backend::Ollama { http, base_url, model } => embed_ollama(http, base_url, model, texts).await?,
            Backend::OpenAi { http, base_url, api_key, model } => {
                embed_openai(http, base_url, api_key, model, texts).await?
            }
        };

        for v in &vectors {
            anyhow::ensure!(
                v.len() == self.dimensions,
                "embedding dimension mismatch: provider returned {} dims, config expects {}",
                v.len(),
                self.dimensions
            );
        }
        Ok(vectors)
    }
}

fn non_empty(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

async fn embed_local(embedder: &Arc<TextEmbedder>, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
    let embedder = Arc::clone(embedder);
    let results = embedder
        .embed(texts, Some(DEFAULT_BATCH_SIZE), None)
        .await
        .context("embedding failed")?;

    results.into_iter().map(|r| r.to_dense().context("converting to dense")).collect()
}

#[derive(Deserialize)]
struct OllamaEmbedResponse {
    embedding: Vec<f32>,
}

async fn embed_ollama(
    http: &reqwest::Client,
    base_url: &str,
    model: &str,
    texts: &[&str],
) -> Result<Vec<Vec<f32>>> {
    let url = format!("{}/api/embeddings", base_url.trim_end_matches('/'));
    let mut out = Vec::with_capacity(texts.len());
    for text in texts {
        let resp = http
            .post(&url)
            .json(&json!({ "model": model, "prompt": text }))
            .send()
            .await
            .context("Ollama embedding request failed")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Ollama embedding error {status}: {body:.300}");
        }
        let parsed: OllamaEmbedResponse = resp.json().await.context("invalid Ollama embedding response")?;
        out.push(parsed.embedding);
    }
    Ok(out)
}

#[derive(Deserialize)]
struct OpenAiEmbedResponse {
    data: Vec<OpenAiEmbedItem>,
}

#[derive(Deserialize)]
struct OpenAiEmbedItem {
    embedding: Vec<f32>,
}

async fn embed_openai(
    http: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    texts: &[&str],
) -> Result<Vec<Vec<f32>>> {
    let url = format!("{}/embeddings", base_url.trim_end_matches('/'));
    let resp = http
        .post(&url)
        .bearer_auth(api_key)
        .json(&json!({ "model": model, "input": texts }))
        .send()
        .await
        .context("OpenAI embedding request failed")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("OpenAI embedding error {status}: {body:.300}");
    }
    let parsed: OpenAiEmbedResponse = resp.json().await.context("invalid OpenAI embedding response")?;
    Ok(parsed.data.into_iter().map(|d| d.embedding).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let cfg = Config::default();
        assert_eq!(cfg.embedding.model_id, "BAAI/bge-small-en-v1.5");
        assert_eq!(cfg.embedding.dimensions, 384);
        assert_eq!(cfg.embedding.provider, "local");
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

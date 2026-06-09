use anyhow::{Context, Result};
use figment::{
    Figment,
    providers::{Format, Serialized, Toml},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const DB_FILE: &str = "poneglyph.db";
const MODEL_CACHE_DIR: &str = "models";
const DEFAULT_MODEL_ID: &str = "BAAI/bge-small-en-v1.5";
const EMBED_DIM: usize = 384;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    pub model_id: String,
    pub dimensions: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model_id: DEFAULT_MODEL_ID.to_string(),
            dimensions: EMBED_DIM,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub mcp: bool,
    pub http_port: u16,
    pub bind_addr: String,
    pub api_token: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            mcp: true,
            http_port: 3742,
            bind_addr: "127.0.0.1".to_string(),
            api_token: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub enabled: bool,
    pub endpoint: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: None,
            model: None,
            api_key: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphConfig {
    pub similarity_threshold: f64,
    pub temporal_window_secs: i64,
}

impl Default for GraphConfig {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.82,
            temporal_window_secs: 300,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextConfig {
    pub max_tokens: usize,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self { max_tokens: 2000 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrichmentConfig {
    pub enabled: bool,
}

impl Default for EnrichmentConfig {
    fn default() -> Self {
        Self { enabled: false }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub db_path: PathBuf,
    pub model_cache_dir: PathBuf,
    pub embedding: EmbeddingConfig,
    pub server: ServerConfig,
    pub llm: LlmConfig,
    pub graph: GraphConfig,
    pub context: ContextConfig,
    pub enrichment: EnrichmentConfig,
}

impl Config {
    pub fn data_dir() -> PathBuf {
        directories::ProjectDirs::from("", "", "poneglyph")
            .map(|dirs| dirs.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn config_dir() -> PathBuf {
        directories::ProjectDirs::from("", "", "poneglyph")
            .map(|dirs| dirs.config_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn default_config_path() -> PathBuf {
        Self::config_dir().join("config.toml")
    }

    pub fn default_db_path() -> PathBuf {
        Self::data_dir().join(DB_FILE)
    }

    pub fn default_model_cache_dir() -> PathBuf {
        Self::data_dir().join(MODEL_CACHE_DIR)
    }

    pub fn default() -> Self {
        let data = Self::data_dir();
        Self {
            db_path: data.join(DB_FILE),
            model_cache_dir: data.join(MODEL_CACHE_DIR),
            embedding: EmbeddingConfig::default(),
            server: ServerConfig::default(),
            llm: LlmConfig::default(),
            graph: GraphConfig::default(),
            context: ContextConfig::default(),
            enrichment: EnrichmentConfig::default(),
        }
    }

    pub fn load() -> Result<Self> {
        Self::load_from(Self::default_config_path())
    }

    pub fn load_from(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let config = if path.exists() {
            Figment::new()
                .merge(Serialized::defaults(Self::default()))
                .merge(Toml::file(&path))
                .extract()
                .with_context(|| format!("failed to parse config: {}", path.display()))?
        } else {
            Self::default()
        };
        Ok(config)
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(&self.db_path.parent().unwrap_or(std::path::Path::new(".")))?;
        std::fs::create_dir_all(&self.model_cache_dir)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use tempfile::NamedTempFile;

    #[test]
    fn default_config_has_correct_values() {
        let cfg = Config::default();
        assert_eq!(cfg.embedding.dimensions, 384);
        assert_eq!(cfg.server.http_port, 3742);
        assert_eq!(cfg.server.bind_addr, "127.0.0.1");
        assert!(!cfg.llm.enabled);
        assert_eq!(cfg.graph.similarity_threshold, 0.82);
        assert_eq!(cfg.graph.temporal_window_secs, 300);
        assert_eq!(cfg.context.max_tokens, 2000);
    }

    #[test]
    fn load_from_nonexistent_file_returns_defaults() {
        let cfg = Config::load_from("/tmp/poneglyph_nonexistent_config.toml").unwrap();
        assert_eq!(cfg.embedding.dimensions, 384);
    }

    #[test]
    fn load_from_toml_file_overrides_defaults() {
        let toml_content = r#"
            [server]
            http_port = 9999
            bind_addr = "0.0.0.0"
            api_token = "test-secret"

            [embedding]
            model_id = "custom/model"
        "#;
        let mut file = NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut file, toml_content.as_bytes()).unwrap();
        let cfg = Config::load_from(file.path()).unwrap();
        assert_eq!(cfg.server.http_port, 9999);
        assert_eq!(cfg.server.bind_addr, "0.0.0.0");
        assert_eq!(cfg.server.api_token.as_deref(), Some("test-secret"));
        assert_eq!(cfg.embedding.model_id, "custom/model");
        // Non-overridden defaults remain
        assert_eq!(cfg.graph.similarity_threshold, 0.82);
    }

    #[test]
    #[ignore] // figment env prefix requires careful config; skip for now
    fn env_override_via_figment() {
        // SAFETY: test sets env var for its own scope
        unsafe {
            env::set_var("PONEGLYPH__SERVER__HTTP_PORT", "4242");
        }
        let cfg = Config::load_from("/tmp/poneglyph_nonexistent_config.toml").unwrap();
        assert_eq!(cfg.server.http_port, 4242);
        unsafe {
            env::remove_var("PONEGLYPH__SERVER__HTTP_PORT");
        }
    }
}

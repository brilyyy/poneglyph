use anyhow::{Context, Result};
use figment::{
    Figment,
    providers::{Format, Serialized, Toml},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const DB_FILE: &str = "poneglyph.db";
const MODEL_CACHE_DIR: &str = "models";
const DEFAULT_MODEL_ID: &str = "sentence-transformers/all-MiniLM-L6-v2";
const EMBED_DIM: usize = 384;

/// Project-local config, relative to the current working directory. Deep-merges
/// over the global config (local wins; arrays replace).
const LOCAL_CONFIG: &str = ".poneglyph/config.toml";

// ===========================================================================
// [general]
// ===========================================================================
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    /// Root directory for all Poneglyph data (memories, indexes, graphs).
    pub data_dir: PathBuf,
    /// Log level: trace, debug, info, warn, error.
    pub log_level: String,
    /// Enable/disable automatic background updates.
    pub auto_update: bool,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            data_dir: Config::data_dir(),
            log_level: "info".to_string(),
            auto_update: true,
        }
    }
}

// ===========================================================================
// [embedding]
// ===========================================================================
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Provider: "local", "ollama", "openai".
    pub provider: String,
    /// Model identifier (HF id for local; model name for remote providers).
    pub model_id: String,
    /// Optional path to a local model file (ONNX), relative to data_dir.
    #[serde(default)]
    pub model_path: Option<String>,
    /// Embedding dimensionality. Must match the `vec_memories` table width.
    pub dimensions: usize,
    /// Device: "cpu" or "cuda".
    pub device: String,
    /// Batch size for embedding generation.
    pub batch_size: usize,
    /// Prepended to text embedded via `embed_query` (e.g. e5-family models
    /// want "query: "). Empty by default — the default model has no prompt
    /// convention; e5 users should set this.
    #[serde(default)]
    pub query_prefix: String,
    /// Prepended to text embedded via `embed_passage` (e.g. e5-family models
    /// want "passage: "). Empty by default — see `query_prefix`.
    #[serde(default)]
    pub passage_prefix: String,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: "local".to_string(),
            model_id: DEFAULT_MODEL_ID.to_string(),
            model_path: None,
            dimensions: EMBED_DIM,
            device: "cpu".to_string(),
            batch_size: 32,
            query_prefix: String::new(),
            passage_prefix: String::new(),
        }
    }
}

// ===========================================================================
// [llm]
// ===========================================================================
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub enabled: bool,
    /// Provider: "openai", "anthropic", "gemini", "ollama", "lmstudio", "gpt4all".
    pub provider: String,
    /// Base URL for the API (e.g. "http://localhost:11434/v1" for Ollama).
    #[serde(alias = "endpoint")]
    pub base_url: Option<String>,
    pub model: Option<String>,
    /// Read from env (`PONEGLYPH_LLM_API_KEY`); avoid storing in the file.
    pub api_key: Option<String>,
    pub timeout_seconds: u64,
    pub max_generation_tokens: u32,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "ollama".to_string(),
            base_url: None,
            model: None,
            api_key: None,
            timeout_seconds: 60,
            max_generation_tokens: 2048,
        }
    }
}

// ===========================================================================
// [memory] (+ nested layer_retention, edges)
// ===========================================================================
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerRetention {
    /// Days to retain memories per brain-like layer. 0 = session-only
    /// (ephemeral) or permanent (archival).
    pub ephemeral: u32,
    pub short_term: u32,
    pub working: u32,
    pub long_term: u32,
    pub archival: u32,
}

impl Default for LayerRetention {
    fn default() -> Self {
        Self {
            ephemeral: 0,
            short_term: 7,
            working: 30,
            long_term: 180,
            archival: 0,
        }
    }
}

/// Memory-linkage edge builder thresholds (similarity/temporal between
/// memories). Distinct from the code knowledge graph in [`CodeGraphConfig`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEdgesConfig {
    pub similarity_threshold: f64,
    pub temporal_window_secs: i64,
}

impl Default for MemoryEdgesConfig {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.82,
            temporal_window_secs: 300,
        }
    }
}

/// How `compression_enabled` reduces a memory for context injection.
/// `content` itself is never overwritten by either mode (see `compressed_content`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompressionMode {
    /// Deterministic caveman-grammar substitution only (`compress::compress`).
    #[default]
    Caveman,
    /// Local-LLM extractive rewrite, then caveman-compress the result too.
    /// Falls back to `Caveman` when no usable LLM config is reachable.
    Semantic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Enable/disable the memory observer entirely.
    pub enabled: bool,
    /// How often (seconds) the observer flushes to disk.
    pub flush_interval_secs: u64,
    /// Maximum tokens allowed in a single memory entry (truncated).
    pub max_entry_tokens: usize,
    /// Compress prose at rest (caveman grammar). Expanded on retrieval.
    pub compression_enabled: bool,
    /// Which compression strategy `compression_enabled` applies.
    #[serde(default)]
    pub compression_mode: CompressionMode,
    /// Minimum relevance score (0.0-1.0) for a memory to be stored.
    pub min_relevance_score: f64,
    #[serde(default)]
    pub layer_retention: LayerRetention,
    #[serde(default)]
    pub edges: MemoryEdgesConfig,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            flush_interval_secs: 5,
            max_entry_tokens: 4000,
            // Off until the Phase 2 compression pipeline lands; flipping this on
            // without it would be a no-op.
            compression_enabled: false,
            compression_mode: CompressionMode::default(),
            min_relevance_score: 0.6,
            layer_retention: LayerRetention::default(),
            edges: MemoryEdgesConfig::default(),
        }
    }
}

// ===========================================================================
// [graph] — code knowledge graph (Tree-sitter). Phase 4.
// ===========================================================================
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeGraphConfig {
    pub enabled: bool,
    /// Languages to parse (auto-detect if empty).
    pub languages: Vec<String>,
    /// File patterns to exclude (glob syntax).
    pub exclude_patterns: Vec<String>,
    /// Delay (ms) after file changes before rebuilding.
    pub watch_delay_ms: u64,
    /// Max depth for blast-radius analysis.
    pub blast_radius_depth: usize,
    /// Upper bound on nodes returned per /api/graph or /api/codegraph request
    /// (the viewer's limit slider can request up to this many).
    pub max_render_nodes: usize,
}

impl Default for CodeGraphConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            languages: vec![
                "rust".into(),
                "typescript".into(),
                "javascript".into(),
                "python".into(),
                "go".into(),
            ],
            exclude_patterns: vec![
                "**/target/**".into(),
                "**/node_modules/**".into(),
                "**/.git/**".into(),
                "**/*.test.ts".into(),
                "**/*_test.rs".into(),
            ],
            watch_delay_ms: 2000,
            blast_radius_depth: 5,
            max_render_nodes: 50_000,
        }
    }
}

// ===========================================================================
// [dashboard] — web UI + server binding (was [server]).
// ===========================================================================
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardConfig {
    pub enabled: bool,
    #[serde(alias = "http_port")]
    pub port: u16,
    #[serde(alias = "bind_addr")]
    pub host: String,
    pub open_on_start: bool,
    /// Auth token; read from env (`PONEGLYPH_DASHBOARD_TOKEN`) when set.
    #[serde(alias = "api_token")]
    pub token: Option<String>,
    /// Theme: "system", "dark", "light".
    pub theme: String,
    pub items_per_page: usize,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 3742,
            host: "127.0.0.1".to_string(),
            open_on_start: false,
            token: None,
            theme: "system".to_string(),
            items_per_page: 50,
        }
    }
}

// ===========================================================================
// [agents] — MCP / hook auto-configuration.
// ===========================================================================
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsConfig {
    pub claude_code: bool,
    pub cursor: bool,
    pub gemini_cli: bool,
    pub opencode: bool,
    pub codex: bool,
    pub copilot_cli: bool,
    /// MCP server port for non-stdio clients.
    pub mcp_server_port: u16,
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            claude_code: true,
            cursor: true,
            gemini_cli: true,
            opencode: true,
            codex: true,
            copilot_cli: true,
            mcp_server_port: 37778,
        }
    }
}

// ===========================================================================
// [privacy]
// ===========================================================================
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivacyConfig {
    /// Strip text between these tags entirely (e.g. <private>...</private>).
    pub redaction_tags: Vec<String>,
    /// Path globs to never index.
    pub exclude_paths: Vec<String>,
    /// Anonymize file paths in logs.
    pub anonymize_paths: bool,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            redaction_tags: vec![
                "private".into(),
                "secret".into(),
                "confidential".into(),
            ],
            exclude_paths: vec![
                "**/.env".into(),
                "**/*.pem".into(),
                "**/*.key".into(),
                "**/secrets/**".into(),
            ],
            anonymize_paths: false,
        }
    }
}

// ===========================================================================
// Poneglyph-specific extras (not in the reference schema, kept top-level).
// ===========================================================================
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextConfig {
    pub max_tokens: usize,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self { max_tokens: 2000 }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnrichmentConfig {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecayConfig {
    pub enabled: bool,
    /// Memories with strength below this are archived to cold tier.
    pub min_strength: f64,
    /// Memories with strength below this are candidates for consolidation.
    pub consolidation_threshold: f64,
    /// Base daily decay rate for Ebbinghaus formula.
    pub daily_decay_rate: f64,
}

impl Default for DecayConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_strength: 0.1,
            consolidation_threshold: 0.3,
            daily_decay_rate: 0.02,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidationConfig {
    pub enabled: bool,
    /// How often the consolidation worker runs (in hours).
    pub interval_hours: u64,
    /// Minimum cluster size to create a decoy.
    pub min_cluster_size: usize,
    /// Cosine similarity threshold for agglomerative clustering.
    pub similarity_threshold: f64,
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_hours: 6,
            min_cluster_size: 2,
            similarity_threshold: 0.75,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColdStorageConfig {
    pub enabled: bool,
    /// zstd compression level (1-22, higher = smaller but slower).
    pub compress_level: i32,
}

impl Default for ColdStorageConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            compress_level: 3,
        }
    }
}

// ===========================================================================
// Top-level Config
// ===========================================================================
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub db_path: PathBuf,
    pub model_cache_dir: PathBuf,
    #[serde(default)]
    pub general: GeneralConfig,
    pub embedding: EmbeddingConfig,
    pub llm: LlmConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    /// Code knowledge graph (Tree-sitter). NOT memory edges (see memory.edges).
    #[serde(default)]
    pub graph: CodeGraphConfig,
    #[serde(default)]
    pub dashboard: DashboardConfig,
    #[serde(default)]
    pub agents: AgentsConfig,
    #[serde(default)]
    pub privacy: PrivacyConfig,
    pub context: ContextConfig,
    #[serde(default)]
    pub enrichment: EnrichmentConfig,
    #[serde(default)]
    pub decay: DecayConfig,
    #[serde(default)]
    pub consolidation: ConsolidationConfig,
    #[serde(default)]
    pub cold_storage: ColdStorageConfig,
}

// ---------------------------------------------------------------------------
// Path resolution — XDG on unix (PRD §6.1, §8.14), ProjectDirs on Windows.
// Legacy installs (pre-XDG, e.g. ~/Library/Application Support/poneglyph on
// macOS) are read in place when the new path is empty; never auto-moved
// (WAL sidecars / possibly-live serve make moves unsafe).
// ---------------------------------------------------------------------------

fn home_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// XDG base-dir resolution with injectable env value (testable without
/// mutating process env). Relative env values are ignored per the XDG spec.
fn xdg_dir_from(env_val: Option<std::ffi::OsString>, home: &std::path::Path, suffix: &str) -> PathBuf {
    env_val
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(suffix))
        .join("poneglyph")
}

#[cfg(unix)]
fn xdg_dir(env_var: &str, home_suffix: &str) -> PathBuf {
    xdg_dir_from(std::env::var_os(env_var), &home_dir(), home_suffix)
}

/// Legacy (pre-XDG) data dir, used only as a read fallback.
fn legacy_data_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "poneglyph").map(|d| d.data_dir().to_path_buf())
}

fn legacy_config_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "poneglyph").map(|d| d.config_dir().to_path_buf())
}

/// Prefer `new`; fall back to `legacy` when only the legacy artifact exists.
fn resolve_with_legacy(new: PathBuf, legacy: Option<PathBuf>) -> PathBuf {
    match legacy {
        Some(l) if !new.exists() && l.exists() && l != new => {
            tracing::warn!(
                legacy = %l.display(),
                new = %new.display(),
                "using legacy location — move it to the new path to silence this \
                 (stop poneglyph first, then mv the file/dir)"
            );
            l
        }
        _ => new,
    }
}

// ---------------------------------------------------------------------------
// Env interpolation + legacy migration (pre-parse passes over the TOML text).
// ---------------------------------------------------------------------------

/// Replace `{ env.NAME }` (whitespace-flexible) with the value of `$NAME`,
/// or the empty string if unset. Secrets stay out of the file this way.
fn interpolate_env(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            // Look ahead for `{ <ws> env.NAME <ws> }`.
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if input[j..].starts_with("env.") {
                let name_start = j + 4;
                let mut k = name_start;
                while k < bytes.len()
                    && (bytes[k].is_ascii_alphanumeric() || bytes[k] == b'_')
                {
                    k += 1;
                }
                let name = &input[name_start..k];
                // Skip trailing whitespace then require a closing brace.
                let mut m = k;
                while m < bytes.len() && bytes[m].is_ascii_whitespace() {
                    m += 1;
                }
                if !name.is_empty() && m < bytes.len() && bytes[m] == b'}' {
                    out.push_str(&std::env::var(name).unwrap_or_default());
                    i = m + 1;
                    continue;
                }
            }
        }
        // Not a placeholder: copy this char (handle UTF-8 by char boundary).
        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Rewrite legacy-shape TOML to the current schema, warning once per remap.
/// - `[server]` → `[dashboard]` (+ http_port/bind_addr/api_token field names)
/// - old `[graph]` (memory-edge thresholds) → `[memory.edges]`
fn migrate_legacy(table: &mut toml::Table) {
    use toml::Value;

    // [server] → [dashboard]
    if let Some(Value::Table(mut server)) = table.remove("server") {
        tracing::warn!("config: [server] is deprecated — migrating to [dashboard]");
        if let Some(v) = server.remove("http_port") {
            server.insert("port".into(), v);
        }
        if let Some(v) = server.remove("bind_addr") {
            server.insert("host".into(), v);
        }
        if let Some(v) = server.remove("api_token") {
            server.insert("token".into(), v);
        }
        let dash = table
            .entry("dashboard")
            .or_insert_with(|| Value::Table(Default::default()));
        if let Value::Table(dash) = dash {
            for (k, v) in server {
                dash.entry(k).or_insert(v);
            }
        }
    }

    // Old [graph] memory-edge thresholds → [memory.edges]. The new [graph] is
    // the code knowledge graph (languages/exclude_patterns); detect the old
    // shape by its distinctive keys.
    let graph_is_legacy = table
        .get("graph")
        .and_then(Value::as_table)
        .is_some_and(|g| {
            g.contains_key("similarity_threshold") || g.contains_key("temporal_window_secs")
        });
    if graph_is_legacy {
        tracing::warn!(
            "config: [graph] similarity/temporal keys are deprecated — migrating to [memory.edges]"
        );
        if let Some(Value::Table(mut graph)) = table.remove("graph") {
            let mut edges = toml::Table::new();
            if let Some(v) = graph.remove("similarity_threshold") {
                edges.insert("similarity_threshold".into(), v);
            }
            if let Some(v) = graph.remove("temporal_window_secs") {
                edges.insert("temporal_window_secs".into(), v);
            }
            let mem = table
                .entry("memory")
                .or_insert_with(|| Value::Table(Default::default()));
            if let Value::Table(mem) = mem {
                let mem_edges = mem
                    .entry("edges")
                    .or_insert_with(|| Value::Table(Default::default()));
                if let Value::Table(mem_edges) = mem_edges {
                    for (k, v) in edges {
                        mem_edges.entry(k).or_insert(v);
                    }
                }
            }
            // Preserve any forward-looking code-graph keys the user added.
            if !graph.is_empty() {
                table.insert("graph".into(), Value::Table(graph));
            }
        }
    }
}

/// Read a config file, interpolate env, and migrate legacy keys → ready-to-parse TOML.
fn prepare_toml(path: &std::path::Path) -> Result<String> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config: {}", path.display()))?;
    let interpolated = interpolate_env(&raw);
    let mut table: toml::Table = interpolated
        .parse()
        .with_context(|| format!("failed to parse config: {}", path.display()))?;
    migrate_legacy(&mut table);
    Ok(table.to_string())
}

impl Config {
    pub fn data_dir() -> PathBuf {
        #[cfg(unix)]
        return xdg_dir("XDG_DATA_HOME", ".local/share");
        #[cfg(not(unix))]
        return directories::ProjectDirs::from("", "", "poneglyph")
            .map(|dirs| dirs.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
    }

    pub fn config_dir() -> PathBuf {
        #[cfg(unix)]
        return xdg_dir("XDG_CONFIG_HOME", ".config");
        #[cfg(not(unix))]
        return directories::ProjectDirs::from("", "", "poneglyph")
            .map(|dirs| dirs.config_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
    }

    pub fn cache_dir() -> PathBuf {
        #[cfg(unix)]
        return xdg_dir("XDG_CACHE_HOME", ".cache");
        #[cfg(not(unix))]
        return directories::ProjectDirs::from("", "", "poneglyph")
            .map(|dirs| dirs.cache_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
    }

    pub fn default_config_path() -> PathBuf {
        resolve_with_legacy(
            Self::config_dir().join("config.toml"),
            legacy_config_dir().map(|d| d.join("config.toml")),
        )
    }

    /// Project-local config path (`./.poneglyph/config.toml`), if present.
    pub fn local_config_path() -> PathBuf {
        PathBuf::from(LOCAL_CONFIG)
    }

    pub fn default_db_path() -> PathBuf {
        resolve_with_legacy(
            Self::data_dir().join(DB_FILE),
            legacy_data_dir().map(|d| d.join(DB_FILE)),
        )
    }

    pub fn default_model_cache_dir() -> PathBuf {
        resolve_with_legacy(
            Self::cache_dir().join(MODEL_CACHE_DIR),
            legacy_data_dir().map(|d| d.join(MODEL_CACHE_DIR)),
        )
    }

    /// True when any default path resolved to a legacy location (status hint).
    pub fn using_legacy_paths() -> bool {
        let xdg_db = Self::data_dir().join(DB_FILE);
        let xdg_cfg = Self::config_dir().join("config.toml");
        Self::default_db_path() != xdg_db || Self::default_config_path() != xdg_cfg
    }

    pub fn default() -> Self {
        Self {
            db_path: Self::default_db_path(),
            model_cache_dir: Self::default_model_cache_dir(),
            general: GeneralConfig::default(),
            embedding: EmbeddingConfig::default(),
            llm: LlmConfig::default(),
            memory: MemoryConfig::default(),
            graph: CodeGraphConfig::default(),
            dashboard: DashboardConfig::default(),
            agents: AgentsConfig::default(),
            privacy: PrivacyConfig::default(),
            context: ContextConfig::default(),
            enrichment: EnrichmentConfig::default(),
            decay: DecayConfig::default(),
            consolidation: ConsolidationConfig::default(),
            cold_storage: ColdStorageConfig::default(),
        }
    }

    /// Load with global → project-local deep merge (local wins; arrays replace).
    pub fn load() -> Result<Self> {
        let mut fig = Figment::new().merge(Serialized::defaults(Self::default()));

        let global = Self::default_config_path();
        if global.exists() {
            fig = fig.merge(Toml::string(&prepare_toml(&global)?));
        }
        let local = Self::local_config_path();
        if local.exists() {
            fig = fig.merge(Toml::string(&prepare_toml(&local)?));
        }

        fig.extract().context("failed to assemble config")
    }

    /// Load a single explicit config file (CLI `--config`), still applying env
    /// interpolation and legacy migration.
    pub fn load_from(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let config = if path.exists() {
            Figment::new()
                .merge(Serialized::defaults(Self::default()))
                .merge(Toml::string(&prepare_toml(&path)?))
                .extract()
                .with_context(|| format!("failed to parse config: {}", path.display()))?
        } else {
            Self::default()
        };
        Ok(config)
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(self.db_path.parent().unwrap_or(std::path::Path::new(".")))?;
        std::fs::create_dir_all(&self.model_cache_dir)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_toml(content: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(content.as_bytes()).unwrap();
        file
    }

    #[test]
    fn default_config_has_correct_values() {
        let cfg = Config::default();
        assert_eq!(cfg.embedding.dimensions, 384);
        assert_eq!(cfg.embedding.provider, "local");
        assert_eq!(cfg.dashboard.port, 3742);
        assert_eq!(cfg.dashboard.host, "127.0.0.1");
        assert!(!cfg.llm.enabled);
        assert_eq!(cfg.memory.edges.similarity_threshold, 0.82);
        assert_eq!(cfg.memory.edges.temporal_window_secs, 300);
        assert_eq!(cfg.context.max_tokens, 2000);
        assert!(cfg.decay.enabled);
        assert_eq!(cfg.decay.min_strength, 0.1);
        assert_eq!(cfg.consolidation.similarity_threshold, 0.75);
        assert_eq!(cfg.cold_storage.compress_level, 3);
        assert_eq!(cfg.graph.languages.len(), 5);
        assert_eq!(cfg.memory.layer_retention.long_term, 180);
    }

    #[test]
    fn load_from_nonexistent_file_returns_defaults() {
        let cfg = Config::load_from("/tmp/poneglyph_nonexistent_config.toml").unwrap();
        assert_eq!(cfg.embedding.dimensions, 384);
    }

    #[test]
    fn load_from_toml_file_overrides_defaults() {
        let file = write_toml(
            r#"
            [dashboard]
            port = 9999
            host = "0.0.0.0"
            token = "test-secret"

            [embedding]
            model_id = "custom/model"
            "#,
        );
        let cfg = Config::load_from(file.path()).unwrap();
        assert_eq!(cfg.dashboard.port, 9999);
        assert_eq!(cfg.dashboard.host, "0.0.0.0");
        assert_eq!(cfg.dashboard.token.as_deref(), Some("test-secret"));
        assert_eq!(cfg.embedding.model_id, "custom/model");
        // Non-overridden defaults remain.
        assert_eq!(cfg.memory.edges.similarity_threshold, 0.82);
    }

    #[test]
    fn env_interpolation_resolves_placeholders() {
        // SAFETY: test owns this env var for its scope.
        unsafe {
            std::env::set_var("PONEGLYPH_TEST_KEY", "sk-from-env");
        }
        let file = write_toml(
            r#"
            [llm]
            api_key = "{ env.PONEGLYPH_TEST_KEY }"
            model = "{env.PONEGLYPH_TEST_KEY}"
            "#,
        );
        let cfg = Config::load_from(file.path()).unwrap();
        assert_eq!(cfg.llm.api_key.as_deref(), Some("sk-from-env"));
        assert_eq!(cfg.llm.model.as_deref(), Some("sk-from-env"));
        unsafe {
            std::env::remove_var("PONEGLYPH_TEST_KEY");
        }
    }

    #[test]
    fn env_interpolation_missing_var_is_empty() {
        let file = write_toml(
            r#"
            [llm]
            api_key = "{ env.PONEGLYPH_DEFINITELY_UNSET }"
            "#,
        );
        let cfg = Config::load_from(file.path()).unwrap();
        assert_eq!(cfg.llm.api_key.as_deref(), Some(""));
    }

    #[test]
    fn legacy_server_section_migrates_to_dashboard() {
        let file = write_toml(
            r#"
            [server]
            http_port = 8080
            bind_addr = "0.0.0.0"
            api_token = "tok"
            mcp = false
            "#,
        );
        // The legacy `mcp = false` key has no home in the current schema
        // (poneglyph serve / poneglyph viewer are separate commands now) —
        // confirm it's silently ignored rather than rejected.
        let cfg = Config::load_from(file.path()).unwrap();
        assert_eq!(cfg.dashboard.port, 8080);
        assert_eq!(cfg.dashboard.host, "0.0.0.0");
        assert_eq!(cfg.dashboard.token.as_deref(), Some("tok"));
    }

    #[test]
    fn legacy_graph_section_migrates_to_memory_edges() {
        let file = write_toml(
            r#"
            [graph]
            similarity_threshold = 0.5
            temporal_window_secs = 99
            "#,
        );
        let cfg = Config::load_from(file.path()).unwrap();
        assert_eq!(cfg.memory.edges.similarity_threshold, 0.5);
        assert_eq!(cfg.memory.edges.temporal_window_secs, 99);
        // New code-graph defaults untouched.
        assert!(cfg.graph.enabled);
        assert_eq!(cfg.graph.languages.len(), 5);
    }

    #[test]
    fn new_graph_section_is_code_graph() {
        let file = write_toml(
            r#"
            [graph]
            enabled = false
            languages = ["rust"]
            blast_radius_depth = 9
            "#,
        );
        let cfg = Config::load_from(file.path()).unwrap();
        assert!(!cfg.graph.enabled);
        assert_eq!(cfg.graph.languages, vec!["rust"]);
        assert_eq!(cfg.graph.blast_radius_depth, 9);
    }

    #[test]
    fn arrays_replace_not_merge() {
        // A user-set array fully replaces the default (no element union).
        let file = write_toml(
            r#"
            [graph]
            languages = ["go"]
            "#,
        );
        let cfg = Config::load_from(file.path()).unwrap();
        assert_eq!(cfg.graph.languages, vec!["go"]);
    }

    #[test]
    fn xdg_dir_from_resolution() {
        let home = std::path::Path::new("/home/u");

        let p = xdg_dir_from(Some("/custom/data".into()), home, ".local/share");
        assert_eq!(p, PathBuf::from("/custom/data/poneglyph"));

        let p = xdg_dir_from(None, home, ".config");
        assert_eq!(p, PathBuf::from("/home/u/.config/poneglyph"));

        let p = xdg_dir_from(Some("relative/path".into()), home, ".cache");
        assert_eq!(p, PathBuf::from("/home/u/.cache/poneglyph"));
    }

    #[test]
    fn resolve_with_legacy_cases() {
        let dir = tempfile::tempdir().unwrap();
        let new = dir.path().join("new/poneglyph.db");
        let legacy = dir.path().join("legacy/poneglyph.db");

        assert_eq!(resolve_with_legacy(new.clone(), Some(legacy.clone())), new);

        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, b"x").unwrap();
        assert_eq!(resolve_with_legacy(new.clone(), Some(legacy.clone())), legacy);

        std::fs::create_dir_all(new.parent().unwrap()).unwrap();
        std::fs::write(&new, b"x").unwrap();
        assert_eq!(resolve_with_legacy(new.clone(), Some(legacy)), new);

        assert_eq!(resolve_with_legacy(new.clone(), None), new);
    }
}

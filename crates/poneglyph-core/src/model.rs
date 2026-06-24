use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Enumerations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    Episodic,
    Semantic,
    Procedural,
    Fact,
    Preference,
    CodeContext,
}

impl std::fmt::Display for MemoryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Episodic => "episodic",
            Self::Semantic => "semantic",
            Self::Procedural => "procedural",
            Self::Fact => "fact",
            Self::Preference => "preference",
            Self::CodeContext => "code_context",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for MemoryType {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "episodic" => Ok(Self::Episodic),
            "semantic" => Ok(Self::Semantic),
            "procedural" => Ok(Self::Procedural),
            "fact" => Ok(Self::Fact),
            "preference" => Ok(Self::Preference),
            "code_context" => Ok(Self::CodeContext),
            _ => Err(anyhow::anyhow!("unknown memory_type: {s}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Explicit,
    Passive,
    Cli,
    Import,
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Explicit => "explicit",
            Self::Passive => "passive",
            Self::Cli => "cli",
            Self::Import => "import",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for Source {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "explicit" => Ok(Self::Explicit),
            "passive" => Ok(Self::Passive),
            "cli" => Ok(Self::Cli),
            "import" => Ok(Self::Import),
            _ => Err(anyhow::anyhow!("unknown source: {s}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeType {
    Explicit,
    Similarity,
    Temporal,
    TagOverlap,
    Relation,
}

impl std::fmt::Display for EdgeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Explicit => "explicit",
            Self::Similarity => "similarity",
            Self::Temporal => "temporal",
            Self::TagOverlap => "tag_overlap",
            Self::Relation => "relation",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for EdgeType {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "explicit" => Ok(Self::Explicit),
            "similarity" => Ok(Self::Similarity),
            "temporal" => Ok(Self::Temporal),
            "tag_overlap" => Ok(Self::TagOverlap),
            "relation" => Ok(Self::Relation),
            _ => Err(anyhow::anyhow!("unknown edge_type: {s}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobType {
    /// No-LLM edge computation (similarity/temporal/tag-overlap) — always on.
    ComputeEdges,
    Summarize,
    ExtractEntities,
    ExtractRelations,
    ScoreImportance,
    /// Token-reduced retrievable rewrite for context injection only — never a substitute
    /// for `memories.content` in recall/FTS/vector search.
    ExtractCompress,
}

impl std::fmt::Display for JobType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::ComputeEdges => "compute_edges",
            Self::Summarize => "summarize",
            Self::ExtractEntities => "extract_entities",
            Self::ExtractRelations => "extract_relations",
            Self::ScoreImportance => "score_importance",
            Self::ExtractCompress => "extract_compress",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for JobType {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "compute_edges" => Ok(Self::ComputeEdges),
            "summarize" => Ok(Self::Summarize),
            "extract_entities" => Ok(Self::ExtractEntities),
            "extract_relations" => Ok(Self::ExtractRelations),
            "score_importance" => Ok(Self::ScoreImportance),
            "extract_compress" => Ok(Self::ExtractCompress),
            _ => Err(anyhow::anyhow!("unknown job_type: {s}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Pending,
    Running,
    Done,
    Failed,
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for JobStatus {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "done" => Ok(Self::Done),
            "failed" => Ok(Self::Failed),
            _ => Err(anyhow::anyhow!("unknown job_status: {s}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Core structs
// ---------------------------------------------------------------------------

/// Storage tier — determines where content lives.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Full content in DB, searchable via dense + FTS.
    #[default]
    Hot,
    /// Full content in DB, accessed occasionally.
    Warm,
    /// Content compressed to .zst file, lazy-loaded on demand.
    Cold,
}

impl std::fmt::Display for Tier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hot => write!(f, "hot"),
            Self::Warm => write!(f, "warm"),
            Self::Cold => write!(f, "cold"),
        }
    }
}

impl std::str::FromStr for Tier {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "hot" => Ok(Self::Hot),
            "warm" => Ok(Self::Warm),
            "cold" => Ok(Self::Cold),
            _ => Err(anyhow::anyhow!("unknown tier: {s}")),
        }
    }
}

/// A single unit of persistent memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    /// UUIDv7 (time-sortable)
    pub id: String,
    pub content: String,
    pub memory_type: MemoryType,
    /// 0.0–1.0
    pub importance: f64,
    /// FK → projects.id, nullable
    pub project_id: Option<String>,
    pub source: Source,
    /// Arbitrary JSON blob (tags, file paths, tool name, etc.)
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub accessed_at: Option<DateTime<Utc>>,
    pub access_count: i64,
    /// True if this is a schema decoy (consolidated cluster summary).
    pub is_decoy: bool,
    /// Storage tier: hot / warm / cold.
    pub tier: Tier,
    /// Ebbinghaus memory strength 0.0–1.0. Decays over time, reinforced on access.
    pub strength: f64,
    /// Path to compressed .zst file when tier=cold, None otherwise.
    pub cold_path: Option<String>,
}

/// A directed edge in the knowledge graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: String,
    pub src_id: String,
    pub dst_id: String,
    pub edge_type: EdgeType,
    /// Predicate text for LLM `relation` edges; None otherwise.
    pub label: Option<String>,
    pub weight: f64,
    pub created_at: DateTime<Utc>,
}

/// A detected or registered project (identified by filesystem path).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    /// Absolute directory path
    pub path: String,
    /// Normalized git remote URL — stable identity across reclones
    pub git_remote: Option<String>,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

/// A background enrichment job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub job_type: JobType,
    pub memory_id: String,
    pub status: JobStatus,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Code knowledge graph (Tree-sitter) — distinct from the memory-linkage
// `Edge`/`EdgeType` above. See core::codegraph.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CgNodeKind {
    Function,
    Method,
    /// Class / struct / interface — call sites care about callability, not
    /// the type-declaration shape, so these share one kind.
    Type,
    Import,
    Test,
}

impl std::fmt::Display for CgNodeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Function => "function",
            Self::Method => "method",
            Self::Type => "type",
            Self::Import => "import",
            Self::Test => "test",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for CgNodeKind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "function" => Ok(Self::Function),
            "method" => Ok(Self::Method),
            "type" => Ok(Self::Type),
            "import" => Ok(Self::Import),
            "test" => Ok(Self::Test),
            _ => Err(anyhow::anyhow!("unknown cg_node kind: {s}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CgEdgeKind {
    Calls,
    Imports,
    Tests,
    /// Class extends/interface implements/Rust trait impl — unified into one
    /// kind, same rationale as `CgNodeKind::Type` unifying class/struct/
    /// interface/trait: callers care about the relationship, not which
    /// language keyword produced it. Points from concrete to abstract
    /// (implementor -> base/interface/trait).
    Extends,
}

impl std::fmt::Display for CgEdgeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Calls => "calls",
            Self::Imports => "imports",
            Self::Tests => "tests",
            Self::Extends => "extends",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for CgEdgeKind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "calls" => Ok(Self::Calls),
            "imports" => Ok(Self::Imports),
            "tests" => Ok(Self::Tests),
            "extends" => Ok(Self::Extends),
            _ => Err(anyhow::anyhow!("unknown cg_edge kind: {s}")),
        }
    }
}

/// A parsed code symbol (function, method, type, import, or test).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CgNode {
    /// Deterministic: `{file}#{start_line}:{name}` — stable across rebuilds
    /// of an unchanged file so re-parsing doesn't churn edge rows.
    pub id: String,
    pub file_path: String,
    pub kind: CgNodeKind,
    pub name: String,
    pub start_line: usize,
    pub end_line: usize,
}

/// A directed edge between two code symbols.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CgEdge {
    pub src_id: String,
    pub dst_id: String,
    pub kind: CgEdgeKind,
}

/// A tracked source file (for incremental re-parsing).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CgFile {
    pub path: String,
    pub language: String,
    pub content_hash: String,
}

// ---------------------------------------------------------------------------
// Input helpers (for create operations)
// ---------------------------------------------------------------------------

/// Parameters for creating a new memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMemory {
    pub content: String,
    #[serde(default = "default_memory_type")]
    pub memory_type: MemoryType,
    #[serde(default = "default_importance")]
    pub importance: f64,
    pub project_path: Option<String>,
    pub tags: Option<Vec<String>>,
    pub source: Option<Source>,
    /// If true, enqueue LLM enrichment jobs in addition to no-LLM edges.
    #[serde(default)]
    pub llm_assist: bool,
}

fn default_memory_type() -> MemoryType {
    MemoryType::Semantic
}

fn default_importance() -> f64 {
    0.5
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn job_type_round_trips_through_string() {
        for jt in [
            JobType::ComputeEdges,
            JobType::Summarize,
            JobType::ExtractEntities,
            JobType::ExtractRelations,
            JobType::ScoreImportance,
            JobType::ExtractCompress,
        ] {
            let s = jt.to_string();
            assert_eq!(JobType::from_str(&s).unwrap(), jt);
        }
    }
}

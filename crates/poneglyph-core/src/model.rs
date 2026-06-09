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
    Summarize,
    ExtractEntities,
    ExtractRelations,
    ScoreImportance,
}

impl std::fmt::Display for JobType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Summarize => "summarize",
            Self::ExtractEntities => "extract_entities",
            Self::ExtractRelations => "extract_relations",
            Self::ScoreImportance => "score_importance",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for JobType {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "summarize" => Ok(Self::Summarize),
            "extract_entities" => Ok(Self::ExtractEntities),
            "extract_relations" => Ok(Self::ExtractRelations),
            "score_importance" => Ok(Self::ScoreImportance),
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

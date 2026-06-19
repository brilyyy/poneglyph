//! MCP tool definitions (PRD §9).
//!
//! Handlers call `poneglyph-core`; embedding runs *before* the store lock is
//! taken so no await happens while holding the mutex, and nothing heavier
//! than persist+index runs on the handler path.

use std::sync::{Arc, Mutex};

use rmcp::{
    ErrorData, ServerHandler,
    handler::server::wrapper::Parameters,
    model::{ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use poneglyph_core::codegraph;
use poneglyph_core::config::Config;
use poneglyph_core::embed::Embedder;
use poneglyph_core::enrich::{self, EnrichHandle};
use poneglyph_core::model::{CgNode, Memory, MemoryType, Source};
use poneglyph_core::retrieve::RecallFilters;
use poneglyph_core::store::Store;

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RememberRequest {
    /// The content to remember.
    pub content: String,
    /// One of: episodic, semantic, procedural, fact, preference, code_context.
    pub memory_type: Option<String>,
    /// Importance 0.0–1.0 (default 0.5).
    pub importance: Option<f64>,
    /// Absolute path of the project this memory belongs to.
    pub project_path: Option<String>,
    /// Free-form tags.
    pub tags: Option<Vec<String>>,
    /// Enqueue LLM enrichment in addition to no-LLM edges (default false).
    pub llm_assist: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecallRequest {
    /// Natural-language search query.
    pub query: String,
    /// Max results (default 10).
    pub limit: Option<usize>,
    /// Filter by memory type.
    pub memory_type: Option<String>,
    /// Filter by project path.
    pub project_path: Option<String>,
    /// Only memories created at/after this ISO-8601 timestamp.
    pub since: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ForgetRequest {
    /// Memory id to delete.
    pub id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateMemoryRequest {
    /// Memory id to update.
    pub id: String,
    /// Replacement content.
    pub new_content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetProjectContextRequest {
    /// Absolute project path.
    pub project_path: String,
    /// Token budget for the context string (default 2000).
    pub max_tokens: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListMemoriesRequest {
    /// Filter by project path.
    pub project_path: Option<String>,
    /// Filter by memory type.
    pub memory_type: Option<String>,
    /// Page size (default 20).
    pub limit: Option<usize>,
    /// Offset for pagination (default 0).
    pub offset: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CodegraphQueryRequest {
    /// `callers_of:<name>`, `callees_of:<name>`, `imports_of:<name>`,
    /// `tests_for:<name>`, or a bare keyword.
    pub query: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CodegraphBlastRadiusRequest {
    /// File path (relative to the graph root) or symbol name.
    pub target: String,
    /// Max traversal depth (defaults to `[graph].blast_radius_depth`).
    pub depth: Option<usize>,
}

// ---------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, JsonSchema)]
pub struct RememberResult {
    pub id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct MemoryView {
    pub id: String,
    pub content: String,
    pub memory_type: String,
    pub importance: f64,
    /// Recall relevance score; 0 when listing.
    pub score: f64,
    pub created_at: String,
    pub metadata: Option<serde_json::Value>,
}

impl MemoryView {
    fn from_memory(m: &Memory, score: f64) -> Self {
        Self {
            id: m.id.clone(),
            content: m.content.clone(),
            memory_type: m.memory_type.to_string(),
            importance: m.importance,
            score,
            created_at: m.created_at.to_rfc3339(),
            metadata: m.metadata.clone(),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RecallResponse {
    pub results: Vec<MemoryView>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ForgetResult {
    pub deleted: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct UpdateMemoryResult {
    pub id: String,
    pub updated: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ProjectContextResult {
    pub context: String,
    pub memory_count: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ListMemoriesResponse {
    pub results: Vec<MemoryView>,
    pub total: i64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CodegraphNodeView {
    pub id: String,
    /// function | method | type | import | test.
    pub kind: String,
    pub name: String,
    pub file_path: String,
    pub start_line: usize,
    pub end_line: usize,
}

impl From<&CgNode> for CodegraphNodeView {
    fn from(n: &CgNode) -> Self {
        Self {
            id: n.id.clone(),
            kind: n.kind.to_string(),
            name: n.name.clone(),
            file_path: n.file_path.clone(),
            start_line: n.start_line,
            end_line: n.end_line,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CodegraphQueryResponse {
    pub results: Vec<CodegraphNodeView>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CodegraphDependentView {
    pub node: CodegraphNodeView,
    pub depth: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CodegraphBlastRadiusResponse {
    pub root: Vec<CodegraphNodeView>,
    pub dependents: Vec<CodegraphDependentView>,
    pub tests: Vec<CodegraphNodeView>,
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

use rmcp::Json;

#[derive(Clone)]
pub struct PoneglyphMcp {
    store: Arc<Mutex<Store>>,
    /// None ⇒ degrade gracefully: FTS-only recall, no vec indexing.
    embedder: Option<Arc<Embedder>>,
    config: Arc<Config>,
    /// Wake-up handle for the background edge worker; edge jobs are still
    /// enqueued without it and drained on the next worker poll.
    enrich: Option<EnrichHandle>,
}

fn internal(e: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(e.to_string(), None)
}

impl PoneglyphMcp {
    pub fn new(store: Arc<Mutex<Store>>, embedder: Option<Arc<Embedder>>, config: Arc<Config>) -> Self {
        Self { store, embedder, config, enrich: None }
    }

    pub fn with_enrich(mut self, handle: EnrichHandle) -> Self {
        self.enrich = Some(handle);
        self
    }

    fn lock_store(&self) -> Result<std::sync::MutexGuard<'_, Store>, ErrorData> {
        self.store.lock().map_err(|_| internal("store mutex poisoned"))
    }

    /// Embed text being stored (memory content/edits).
    async fn embed_passage_or_none(&self, text: &str) -> Result<Option<Vec<f32>>, ErrorData> {
        match &self.embedder {
            Some(e) => e.embed_passage(text).await.map(Some).map_err(internal),
            None => Ok(None),
        }
    }

    /// Embed a search query.
    async fn embed_query_or_none(&self, text: &str) -> Result<Option<Vec<f32>>, ErrorData> {
        match &self.embedder {
            Some(e) => e.embed_query(text).await.map(Some).map_err(internal),
            None => Ok(None),
        }
    }

    /// Resolve a project path filter to a project id. `Ok(None)` means the
    /// project is unknown, i.e. the filtered result set is empty.
    fn project_id_for(&self, store: &Store, path: &str) -> Result<Option<String>, ErrorData> {
        Ok(store.get_project(path).map_err(internal)?.map(|p| p.id))
    }
}

#[tool_router]
impl PoneglyphMcp {
    #[tool(description = "Store a memory for later recall. Use for durable facts, decisions, preferences, and project context worth keeping across sessions.")]
    pub async fn remember(
        &self,
        Parameters(req): Parameters<RememberRequest>,
    ) -> Result<Json<RememberResult>, ErrorData> {
        let memory_type: MemoryType = req
            .memory_type
            .as_deref()
            .unwrap_or("semantic")
            .parse()
            .map_err(|e| ErrorData::invalid_params(format!("{e}"), None))?;
        let importance = req.importance.unwrap_or(0.5).clamp(0.0, 1.0);

        // Embed before taking the lock (no await under the mutex).
        let embedding = self.embed_passage_or_none(&req.content).await?;

        let id = {
            let store = self.lock_store()?;

            let project_id = match req.project_path.as_deref() {
                Some(path) => Some(
                    poneglyph_core::project::detect_project(&store, path)
                        .map_err(internal)?
                        .id,
                ),
                None => None,
            };

            let metadata = req.tags.as_ref().filter(|t| !t.is_empty()).map(|tags| {
                serde_json::json!({ "tags": tags })
            });

            let mem = store
                .create_memory(
                    &req.content,
                    memory_type,
                    importance,
                    Source::Explicit,
                    project_id.as_deref(),
                    metadata.as_ref(),
                )
                .map_err(internal)?;

            store.index_fts(&mem.id, &req.content).map_err(internal)?;
            if let Some(vec) = &embedding {
                store.index_embedding(&mem.id, vec).map_err(internal)?;
            }

            // Edge computation is queued, never run here (§8.4).
            enrich::enqueue_compute_edges(&store, &mem.id).map_err(internal)?;

            // llm_assist enqueues enrichment, it does not run it (§9). Gated
            // on config so a disabled install never even creates LLM jobs.
            if req.llm_assist.unwrap_or(false)
                && self.config.enrichment.enabled
                && self.config.llm.enabled
            {
                enrich::enqueue_llm_jobs(&store, &mem.id).map_err(internal)?;
            }

            // Compression is orthogonal to llm_assist/enrichment.
            if self.config.memory.compression_enabled {
                enrich::enqueue_compression(&store, &mem.id, self.config.memory.compression_mode)
                    .map_err(internal)?;
            }

            mem.id
        };

        if let Some(h) = &self.enrich {
            h.notify();
        }

        Ok(Json(RememberResult { id }))
    }

    #[tool(description = "Search stored memories with hybrid (semantic + keyword + graph) retrieval.")]
    pub async fn recall(
        &self,
        Parameters(req): Parameters<RecallRequest>,
    ) -> Result<Json<RecallResponse>, ErrorData> {
        let limit = req.limit.unwrap_or(10).clamp(1, 100);
        let query_vec = self.embed_query_or_none(&req.query).await?;

        let store = self.lock_store()?;

        let project_id = match req.project_path.as_deref() {
            Some(path) => match self.project_id_for(&store, path)? {
                Some(id) => Some(id),
                // Unknown project ⇒ nothing can match the filter.
                None => return Ok(Json(RecallResponse { results: vec![] })),
            },
            None => None,
        };

        let filters = RecallFilters {
            memory_type: req.memory_type.clone(),
            project_id,
            since: req.since.clone(),
            tag: None,
        };

        let results = poneglyph_core::retrieve::recall(
            &store.conn,
            query_vec.as_deref(),
            &req.query,
            &filters,
            limit,
        )
        .map_err(internal)?;

        Ok(Json(RecallResponse {
            results: results
                .iter()
                .map(|r| MemoryView::from_memory(&r.memory, r.score))
                .collect(),
        }))
    }

    #[tool(description = "Permanently delete a memory by id.")]
    pub async fn forget(
        &self,
        Parameters(req): Parameters<ForgetRequest>,
    ) -> Result<Json<ForgetResult>, ErrorData> {
        let store = self.lock_store()?;
        let deleted = store.delete_memory(&req.id).map_err(internal)?;
        Ok(Json(ForgetResult { deleted }))
    }

    #[tool(description = "Replace the content of an existing memory (re-indexed automatically).")]
    pub async fn update_memory(
        &self,
        Parameters(req): Parameters<UpdateMemoryRequest>,
    ) -> Result<Json<UpdateMemoryResult>, ErrorData> {
        // Re-embed before locking.
        let embedding = self.embed_passage_or_none(&req.new_content).await?;

        let updated = {
            let store = self.lock_store()?;
            let updated = store
                .update_memory(&req.id, &req.new_content)
                .map_err(internal)?;

            if updated {
                store.index_fts(&req.id, &req.new_content).map_err(internal)?;
                if let Some(vec) = &embedding {
                    store.index_embedding(&req.id, vec).map_err(internal)?;
                }
                // Content changed ⇒ similarity edges may have changed.
                enrich::enqueue_compute_edges(&store, &req.id).map_err(internal)?;
                // Drop any cached compression for the old content — stale
                // otherwise, since nothing here regenerates it.
                store.clear_compressed_content(&req.id).map_err(internal)?;
            }
            updated
        };

        if updated && let Some(h) = &self.enrich {
            h.notify();
        }

        Ok(Json(UpdateMemoryResult { id: req.id, updated }))
    }

    #[tool(description = "Get a ranked context string of stored memories for a project, for injection into the session.")]
    pub async fn get_project_context(
        &self,
        Parameters(req): Parameters<GetProjectContextRequest>,
    ) -> Result<Json<ProjectContextResult>, ErrorData> {
        let max_tokens = req.max_tokens.unwrap_or(self.config.context.max_tokens);
        let store = self.lock_store()?;
        let (context, memory_count) =
            poneglyph_core::project::get_project_context(&store, &req.project_path, max_tokens)
                .map_err(internal)?;
        Ok(Json(ProjectContextResult { context, memory_count }))
    }

    #[tool(description = "List stored memories with optional project/type filters and pagination.")]
    pub async fn list_memories(
        &self,
        Parameters(req): Parameters<ListMemoriesRequest>,
    ) -> Result<Json<ListMemoriesResponse>, ErrorData> {
        let limit = req.limit.unwrap_or(20).clamp(1, 200);
        let offset = req.offset.unwrap_or(0);

        let store = self.lock_store()?;

        let project_id = match req.project_path.as_deref() {
            Some(path) => match self.project_id_for(&store, path)? {
                Some(id) => Some(id),
                None => {
                    return Ok(Json(ListMemoriesResponse { results: vec![], total: 0 }));
                }
            },
            None => None,
        };

        let (memories, total) = store
            .list_memories(project_id.as_deref(), req.memory_type.as_deref(), limit, offset)
            .map_err(internal)?;

        Ok(Json(ListMemoriesResponse {
            results: memories
                .iter()
                .map(|m| MemoryView::from_memory(m, 0.0))
                .collect(),
            total,
        }))
    }

    #[tool(description = "Query the code knowledge graph: callers_of:<name>, callees_of:<name>, imports_of:<name>, tests_for:<name>, path:<from>..<to> (shortest call/import chain between two symbols), or a bare keyword search. Requires `poneglyph graph init` to have been run.")]
    pub async fn codegraph_query(
        &self,
        Parameters(req): Parameters<CodegraphQueryRequest>,
    ) -> Result<Json<CodegraphQueryResponse>, ErrorData> {
        let store = self.lock_store()?;
        let query = codegraph::parse_query(&req.query);
        let results = codegraph::run_query(&store, &query).map_err(internal)?;
        Ok(Json(CodegraphQueryResponse { results: results.iter().map(CodegraphNodeView::from).collect() }))
    }

    #[tool(description = "Recursive caller/importer/test trace from a file or symbol in the code knowledge graph — what breaks if this changes. Requires `poneglyph graph init` to have been run.")]
    pub async fn codegraph_blast_radius(
        &self,
        Parameters(req): Parameters<CodegraphBlastRadiusRequest>,
    ) -> Result<Json<CodegraphBlastRadiusResponse>, ErrorData> {
        let depth = req.depth.unwrap_or(self.config.graph.blast_radius_depth);
        let store = self.lock_store()?;
        let report = codegraph::blast_radius(&store, &req.target, depth).map_err(internal)?;
        Ok(Json(CodegraphBlastRadiusResponse {
            root: report.root.iter().map(CodegraphNodeView::from).collect(),
            dependents: report
                .dependents
                .iter()
                .map(|d| CodegraphDependentView { node: CodegraphNodeView::from(&d.node), depth: d.depth })
                .collect(),
            tests: report.tests.iter().map(CodegraphNodeView::from).collect(),
        }))
    }
}

#[tool_handler]
impl ServerHandler for PoneglyphMcp {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.instructions = Some(
            "poneglyph is a local persistent memory engine with a code knowledge graph. \
             Use `remember` to store durable facts/decisions/preferences, `recall` to search \
             past memories, and `get_project_context` at session start to load project memory. \
             For any \"find/explore/what relates to X\" question about code, call `codegraph_query` \
             FIRST — even a bare keyword (no prefix) runs a graph-backed name search, faster and \
             cheaper than scanning directories file-by-file on a large codebase. Use \
             callers_of:/callees_of:/imports_of:/tests_for:/path:<a>..<b> for structural questions \
             and `codegraph_blast_radius` for impact analysis. Only fall back to grep/glob when the \
             graph returns nothing. Requires `poneglyph graph init` to have been run once."
                .into(),
        );
        info
    }
}

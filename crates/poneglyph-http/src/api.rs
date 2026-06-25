//! Viewer API endpoints (PRD §8.8).
//!
//! Handlers follow the MCP discipline: embed before locking the store,
//! never await under the mutex, enqueue edge work instead of computing it.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use poneglyph_core::codegraph;
use poneglyph_core::enrich;
use poneglyph_core::model::{CgEdge, CgNode, Edge, Memory};
use poneglyph_core::retrieve::RecallFilters;
use poneglyph_core::store::Store;

use crate::error::{ApiError, ApiJson, ApiResult};
use crate::state::AppState;

/// Resolve a project-path filter. `Ok(None)` ⇒ unknown project ⇒ empty results.
fn project_id_for(store: &Store, path: &str) -> Result<Option<String>, ApiError> {
    Ok(store.get_project(path)?.map(|p| p.id))
}

// ---------------------------------------------------------------------------
// /api/memories
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ListQuery {
    pub project_path: Option<String>,
    #[serde(rename = "type")]
    pub memory_type: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Serialize)]
pub struct ListResponse {
    pub results: Vec<Memory>,
    pub total: i64,
}

pub async fn list_memories(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> ApiResult<ListResponse> {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let offset = q.offset.unwrap_or(0);

    let store = state.lock_store()?;
    let project_id = match q.project_path.as_deref() {
        Some(path) => match project_id_for(&store, path)? {
            Some(id) => Some(id),
            None => return Ok(Json(ListResponse { results: vec![], total: 0 })),
        },
        None => None,
    };

    let (results, total) =
        store.list_memories(project_id.as_deref(), q.memory_type.as_deref(), limit, offset)?;
    Ok(Json(ListResponse { results, total }))
}

#[derive(Serialize)]
pub struct MemoryDetail {
    #[serde(flatten)]
    pub memory: Memory,
    pub edges: Vec<Edge>,
    /// For a decoy: the memories it was consolidated from.
    pub children: Vec<Memory>,
    /// For a consolidated child: the decoy it was folded into, if any.
    pub parent: Option<Memory>,
}

pub async fn get_memory(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<MemoryDetail> {
    let store = state.lock_store()?;
    let memory = store
        .get_memory(&id)?
        .ok_or_else(|| ApiError::not_found(format!("memory not found: {id}")))?;
    let edges = store.get_edges_for_memory(&id)?;
    let children = if memory.is_decoy { store.get_decoy_children(&id)? } else { Vec::new() };
    let parent = store.get_child_decoy(&id)?;
    Ok(Json(MemoryDetail { memory, edges, children, parent }))
}

#[derive(Deserialize)]
pub struct PatchMemoryBody {
    pub new_content: String,
}

pub async fn patch_memory(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<PatchMemoryBody>,
) -> ApiResult<Value> {
    if body.new_content.trim().is_empty() {
        return Err(ApiError::bad_request("new_content must be non-empty"));
    }

    // Re-embed before locking.
    let embedding = state.embed_passage_or_none(&body.new_content).await?;

    let updated = {
        let store = state.lock_store()?;
        let updated = store.update_memory(&id, &body.new_content)?;
        if updated {
            store.index_fts(&id, &body.new_content)?;
            if let Some(vec) = &embedding {
                store.index_embedding(&id, vec)?;
            }
            enrich::enqueue_compute_edges(&store, &id)?;
            // Drop any cached compression for the old content — stale
            // otherwise, since nothing here regenerates it.
            store.clear_compressed_content(&id)?;
        }
        updated
    };

    if !updated {
        return Err(ApiError::not_found(format!("memory not found: {id}")));
    }
    if let Some(h) = &state.enrich {
        h.notify();
    }
    Ok(Json(json!({ "id": id, "updated": true })))
}

pub async fn delete_memory(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Value> {
    let store = state.lock_store()?;
    let deleted = store.delete_memory(&id)?;
    if !deleted {
        return Err(ApiError::not_found(format!("memory not found: {id}")));
    }
    Ok(Json(json!({ "deleted": true })))
}

// ---------------------------------------------------------------------------
// /api/search
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: String,
    pub limit: Option<usize>,
    #[serde(rename = "type")]
    pub memory_type: Option<String>,
    pub project_path: Option<String>,
}

#[derive(Serialize)]
pub struct SearchHit {
    #[serde(flatten)]
    pub memory: Memory,
    pub score: f64,
}

pub async fn search(
    State(state): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Value> {
    if q.q.trim().is_empty() {
        return Err(ApiError::bad_request("query parameter `q` must be non-empty"));
    }
    let limit = q.limit.unwrap_or(10).clamp(1, 100);
    let query_vec = state.embed_query_or_none(&q.q).await?;

    let store = state.lock_store()?;
    let project_id = match q.project_path.as_deref() {
        Some(path) => match project_id_for(&store, path)? {
            Some(id) => Some(id),
            None => return Ok(Json(json!({ "results": [] }))),
        },
        None => None,
    };

    let filters = RecallFilters {
        memory_type: q.memory_type.clone(),
        project_id,
        since: None,
        tag: None,
    };
    let results = poneglyph_core::retrieve::recall(
        &store.conn,
        query_vec.as_deref(),
        &q.q,
        &filters,
        limit,
        &state.config.retrieval,
    )?;

    let hits: Vec<SearchHit> = results
        .into_iter()
        .map(|r| SearchHit { memory: r.memory, score: r.score })
        .collect();
    Ok(Json(json!({ "results": hits })))
}

// ---------------------------------------------------------------------------
// /api/graph
// ---------------------------------------------------------------------------

/// Edges below this weight (e.g. single-generic-tag overlaps) are noise in
/// the viewer; drop them by default but let callers opt back in to raw data.
const DEFAULT_MIN_EDGE_WEIGHT: f64 = 0.4;

#[derive(Deserialize)]
pub struct GraphQuery {
    pub focus: Option<String>,
    pub depth: Option<u32>,
    pub limit: Option<usize>,
    pub min_weight: Option<f64>,
}

#[derive(Serialize)]
pub struct GraphResponse {
    pub nodes: Vec<Memory>,
    pub edges: Vec<Edge>,
    /// True memory/edge counts in the store, regardless of `limit` — lets
    /// the viewer show "showing X of Y" instead of silently sampling.
    pub total_nodes: i64,
    pub total_edges: i64,
}

pub async fn graph(
    State(state): State<AppState>,
    Query(q): Query<GraphQuery>,
) -> ApiResult<GraphResponse> {
    let depth = q.depth.unwrap_or(1).clamp(1, 5);
    let limit = q.limit.unwrap_or(500).clamp(1, state.config.graph.max_render_nodes);
    let min_weight = q.min_weight.unwrap_or(DEFAULT_MIN_EDGE_WEIGHT).clamp(0.0, 1.0);

    let store = state.lock_store()?;
    let (nodes, edges) = match q.focus.as_deref() {
        Some(focus) => {
            if store.get_memory(focus)?.is_none() {
                return Err(ApiError::not_found(format!("memory not found: {focus}")));
            }
            store.graph_neighborhood(focus, depth, limit, min_weight)?
        }
        None => store.graph_sample(limit, min_weight)?,
    };
    let stats = store.stats()?;
    Ok(Json(GraphResponse { nodes, edges, total_nodes: stats.memory_count, total_edges: stats.edge_count }))
}

// ---------------------------------------------------------------------------
// /api/codegraph
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CodegraphQuery {
    /// Absolute path of the `graph init`'d project to browse.
    pub project_path: String,
    /// File path (relative to the graph root) or symbol name to center on.
    pub focus: Option<String>,
    pub depth: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Serialize)]
pub struct CodegraphResponse {
    pub nodes: Vec<CgNode>,
    pub edges: Vec<CgEdge>,
    /// True node/edge counts in the code graph, regardless of `limit`.
    pub total_nodes: i64,
    pub total_edges: i64,
    /// True if a file change is still awaiting a debounced graph rebuild.
    pub stale: bool,
}

pub async fn codegraph_graph(
    State(state): State<AppState>,
    Query(q): Query<CodegraphQuery>,
) -> ApiResult<CodegraphResponse> {
    let store = state.lock_store()?;
    let project = poneglyph_core::project::detect_project(&store, &q.project_path)?;

    let (nodes, edges) = match q.focus.as_deref() {
        Some(focus) => {
            let depth = q.depth.unwrap_or(state.config.graph.blast_radius_depth).clamp(1, 10);
            let report = codegraph::blast_radius(&store, &project.id, focus, depth)?;
            let mut nodes = report.root;
            nodes.extend(report.dependents.into_iter().map(|d| d.node));
            nodes.extend(report.tests);
            let id_vec: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
            let edges = store.cg_edges_for_nodes(&project.id, &id_vec)?;
            (nodes, edges)
        }
        None => {
            let limit = q.limit.unwrap_or(500).clamp(1, state.config.graph.max_render_nodes);
            let nodes = store.cg_all_nodes(&project.id, Some(limit))?;
            let id_vec: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
            let edges = store.cg_edges_for_nodes(&project.id, &id_vec)?;
            (nodes, edges)
        }
    };

    let (_files, total_nodes, total_edges) = store.cg_stats(&project.id)?;
    let stale = state.is_graph_dirty(&project.id);
    Ok(Json(CodegraphResponse { nodes, edges, total_nodes, total_edges, stale }))
}

#[derive(Deserialize)]
pub struct CodegraphStatsQuery {
    pub project_path: String,
}

pub async fn codegraph_stats(
    State(state): State<AppState>,
    Query(q): Query<CodegraphStatsQuery>,
) -> ApiResult<Value> {
    let store = state.lock_store()?;
    let project = poneglyph_core::project::detect_project(&store, &q.project_path)?;
    let (files, nodes, edges) = store.cg_stats(&project.id)?;
    let stale = state.is_graph_dirty(&project.id);
    Ok(Json(json!({ "files": files, "nodes": nodes, "edges": edges, "stale": stale })))
}

#[derive(Deserialize)]
pub struct CodegraphExploreQuery {
    pub project_path: String,
    /// File path (relative to the graph root) or symbol name to explore.
    pub target: String,
    pub depth: Option<usize>,
}

/// Everything about a symbol in one call: source snippet, direct
/// callers/callees, supertypes/subtypes, covering tests, and the bounded
/// blast radius. Mirrors the `codegraph_explore` MCP tool.
pub async fn codegraph_explore(
    State(state): State<AppState>,
    Query(q): Query<CodegraphExploreQuery>,
) -> ApiResult<Value> {
    let store = state.lock_store()?;
    let project = poneglyph_core::project::detect_project(&store, &q.project_path)?;
    let depth = q.depth.unwrap_or(state.config.graph.blast_radius_depth).clamp(1, 10);
    let report = codegraph::explore(&store, &project.id, std::path::Path::new(&project.path), &q.target, depth)?;
    let mut body = serde_json::to_value(report).map_err(ApiError::internal)?;
    body["stale"] = json!(state.is_graph_dirty(&project.id));
    Ok(Json(body))
}

#[derive(Deserialize)]
pub struct CodegraphSearchQuery {
    pub project_path: String,
    /// Bare keyword (substring search) or a prefixed query like
    /// `callers_of:<name>`, `path:<from>..<to>`, etc.
    pub q: String,
}

pub async fn codegraph_search(
    State(state): State<AppState>,
    Query(q): Query<CodegraphSearchQuery>,
) -> ApiResult<Value> {
    let store = state.lock_store()?;
    let project = poneglyph_core::project::detect_project(&store, &q.project_path)?;
    let query = codegraph::parse_query(&q.q);
    let results = codegraph::run_query(&store, &project.id, &query)?;
    let stale = state.is_graph_dirty(&project.id);
    Ok(Json(json!({ "results": results, "stale": stale })))
}

// ---------------------------------------------------------------------------
// /api/token-savings — estimated prose-compression savings (PRD: caveman
// grammar). `[memory].compression_enabled` is off by default and not yet
// applied at rest (see core::compress), so this samples stored content and
// runs the real compressor on demand rather than reporting a fake number.
// ---------------------------------------------------------------------------

pub async fn token_savings(State(state): State<AppState>) -> ApiResult<Value> {
    let store = state.lock_store()?;
    let (memories, _) = store.list_memories(None, None, 200, 0)?;

    let mut original_bytes = 0usize;
    let mut compressed_bytes = 0usize;
    for m in &memories {
        original_bytes += m.content.len();
        compressed_bytes += poneglyph_core::compress::compress(&m.content).len();
    }
    let savings_pct =
        if original_bytes > 0 { 100.0 * (1.0 - compressed_bytes as f64 / original_bytes as f64) } else { 0.0 };

    Ok(Json(json!({
        "sampled_memories": memories.len(),
        "original_bytes": original_bytes,
        "compressed_bytes": compressed_bytes,
        "savings_pct": savings_pct,
        "compression_enabled": state.config.memory.compression_enabled,
    })))
}

// ---------------------------------------------------------------------------
// /api/agents-status — wiring status for `[agents]`: config flag vs whether
// `poneglyph init` actually found the agent installed on this machine.
// ---------------------------------------------------------------------------

fn agent_entry(enabled: bool, marker_dir: Option<std::path::PathBuf>) -> Value {
    json!({ "enabled": enabled, "detected": marker_dir.is_some_and(|d| d.exists()) })
}

pub async fn agents_status(State(state): State<AppState>) -> ApiResult<Value> {
    let home = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf());
    let agents = &state.config.agents;
    let copilot_home = std::env::var_os("COPILOT_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".copilot")));

    Ok(Json(json!({
        "claude_code": agent_entry(agents.claude_code, home.as_ref().map(|h| h.join(".claude"))),
        "cursor": agent_entry(agents.cursor, home.as_ref().map(|h| h.join(".cursor"))),
        "gemini_cli": agent_entry(agents.gemini_cli, home.as_ref().map(|h| h.join(".gemini"))),
        "opencode": agent_entry(agents.opencode, home.as_ref().map(|h| h.join(".config/opencode"))),
        "codex": agent_entry(agents.codex, home.as_ref().map(|h| h.join(".codex"))),
        "copilot_cli": agent_entry(agents.copilot_cli, copilot_home),
    })))
}

// ---------------------------------------------------------------------------
// /api/services-status — MCP engine / LLM / viewer up-down for the status
// panel. All three probes run server-side so the browser only ever talks to
// its own origin (avoids CORS to the engine port or the LLM's host).
// ---------------------------------------------------------------------------

pub async fn services_status(State(state): State<AppState>) -> ApiResult<Value> {
    let config = &state.config;

    let mcp_port = config.agents.mcp_server_port;
    let mcp_up = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{mcp_port}/health"))
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await
        .is_ok_and(|r| r.status().is_success());

    let llm = poneglyph_core::llm::health(&config.llm).await;

    Ok(Json(json!({
        "mcp": { "up": mcp_up, "port": mcp_port },
        "llm": {
            "enabled": config.llm.enabled,
            "up": llm.reachable,
            "provider": config.llm.provider,
            "model": config.llm.model,
            "base_url": config.llm.base_url,
            "status": llm.status,
        },
        "viewer": { "up": true, "port": config.dashboard.port },
    })))
}

// ---------------------------------------------------------------------------
// /api/activity — live engine work for the activity panel: in-flight phases
// (enrich/consolidate/graph_build) + outstanding job queue grouped by type +
// projects awaiting a graph rebuild. Polled by the viewer when live-tracking.
// ---------------------------------------------------------------------------

pub async fn activity(State(state): State<AppState>) -> ApiResult<Value> {
    let phases = state.activity.as_ref().map(|a| a.snapshot()).unwrap_or_default();

    let (mut running, mut pending) = (serde_json::Map::new(), serde_json::Map::new());
    {
        let store = state.lock_store()?;
        for (job_type, status, count) in store.job_activity()? {
            let bucket = if status == "running" { &mut running } else { &mut pending };
            bucket.insert(job_type, json!(count));
        }
    }

    let dirty_projects: Vec<String> = state
        .graph_dirty
        .as_ref()
        .and_then(|d| d.lock().ok().map(|d| d.iter().cloned().collect()))
        .unwrap_or_default();

    Ok(Json(json!({
        "phases": phases,
        "jobs": { "running": running, "pending": pending },
        "graph": { "dirty_projects": dirty_projects },
        "generated_at": chrono::Utc::now().to_rfc3339(),
    })))
}

// ---------------------------------------------------------------------------
// /api/context — zero-LLM session context injection (PRD §8.10)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ContextQuery {
    pub project_path: String,
    pub max_tokens: Option<usize>,
}

pub async fn project_context(
    State(state): State<AppState>,
    Query(q): Query<ContextQuery>,
) -> ApiResult<Value> {
    if q.project_path.trim().is_empty() {
        return Err(ApiError::bad_request("project_path must be non-empty"));
    }
    let max_tokens = q
        .max_tokens
        .unwrap_or(state.config.context.max_tokens)
        .clamp(1, 32_000);

    let store = state.lock_store()?;
    let (context, memory_count) =
        poneglyph_core::project::get_project_context(&store, &q.project_path, max_tokens)?;
    Ok(Json(json!({ "context": context, "memory_count": memory_count })))
}

// ---------------------------------------------------------------------------
// /api/enrich — file-level context for OpenCode's file enrichment layer:
// memories mentioning the file + codegraph nodes defined in it.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct EnrichQuery {
    /// Absolute file path to enrich.
    pub file_path: String,
    /// Absolute project path.
    pub project_path: String,
    /// Token budget for the context string (default 1000).
    pub max_tokens: Option<usize>,
}

pub async fn enrich(
    State(state): State<AppState>,
    Query(q): Query<EnrichQuery>,
) -> ApiResult<Value> {
    if q.file_path.trim().is_empty() {
        return Err(ApiError::bad_request("file_path must be non-empty"));
    }
    if q.project_path.trim().is_empty() {
        return Err(ApiError::bad_request("project_path must be non-empty"));
    }
    let max_tokens = q.max_tokens.unwrap_or(1000).clamp(1, 16_000);

    let store = state.lock_store()?;

    // 1. Memories mentioning this file (FTS on file path basename).
    let basename = std::path::Path::new(&q.file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&q.file_path);
    let project_id = project_id_for(&store, &q.project_path)?;
    let filters = RecallFilters {
        memory_type: None,
        project_id: project_id.clone(),
        since: None,
        tag: None,
    };
    let memories = poneglyph_core::retrieve::recall(
        &store.conn,
        None,
        basename,
        &filters,
        10,
        &state.config.retrieval,
    )?;

    // 2. Codegraph nodes in this file.
    let code_nodes = match &project_id {
        Some(pid) => {
            // Use relative path for codegraph lookup.
            let root = store
                .get_project(&q.project_path)?
                .map(|p| std::path::PathBuf::from(&p.path));
            let rel = root
                .as_ref()
                .and_then(|r| std::path::Path::new(&q.file_path).strip_prefix(r).ok())
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| q.file_path.clone());
            store.cg_nodes_in_file(pid, &rel).unwrap_or_default()
        }
        None => vec![],
    };

    // 3. Assemble context string, truncated to max_tokens.
    let mut parts = Vec::new();
    if !memories.is_empty() {
        parts.push("## Related memories".to_string());
        for r in &memories {
            parts.push(format!("- {}", r.memory.content));
        }
    }
    if !code_nodes.is_empty() {
        parts.push("## Code in this file".to_string());
        for n in &code_nodes {
            parts.push(format!("- {} {} ({}:{})", n.kind, n.name, n.file_path, n.start_line));
        }
    }

    let context = parts.join("\n");
    // ponytail: rough char-based truncation instead of tokenizer — good enough
    // for a context hint, max_tokens is a soft budget.
    let truncated = if context.len() > max_tokens * 4 {
        &context[..max_tokens * 4]
    } else {
        &context
    };

    Ok(Json(json!({
        "context": truncated,
        "memory_count": memories.len(),
        "node_count": code_nodes.len(),
    })))
}

// ---------------------------------------------------------------------------
// /api/timeline
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct TimelineQuery {
    pub project_path: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub gap_secs: Option<i64>,
    pub memory_type: Option<String>,
    pub source: Option<String>,
}

#[derive(Serialize)]
pub struct TimelineSession {
    pub session_id: Option<String>,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub started_at: String,
    pub ended_at: String,
    pub memory_count: usize,
    pub memories: Vec<Memory>,
    pub duration_secs: i64,
    pub type_counts: std::collections::HashMap<String, usize>,
    pub source_counts: std::collections::HashMap<String, usize>,
    pub avg_strength: f64,
}

#[derive(Serialize)]
pub struct TimelineResponse {
    pub sessions: Vec<TimelineSession>,
    pub total: i64,
}

pub async fn timeline(
    State(state): State<AppState>,
    Query(q): Query<TimelineQuery>,
) -> ApiResult<TimelineResponse> {
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let offset = q.offset.unwrap_or(0);
    let gap_secs = q.gap_secs.unwrap_or(1800).clamp(60, 86400);

    let store = state.lock_store()?;
    let project_id = match q.project_path.as_deref() {
        Some(path) => match project_id_for(&store, path)? {
            Some(id) => Some(id),
            None => return Ok(Json(TimelineResponse { sessions: vec![], total: 0 })),
        },
        None => None,
    };

    // Build project name map once.
    let projects = store.list_projects()?;
    let project_names: std::collections::HashMap<&str, &str> = projects
        .iter()
        .map(|p| (p.id.as_str(), p.name.as_str()))
        .collect();

    let (groups, total) = store.list_sessions(project_id.as_deref(), gap_secs, limit, offset)?;

    let mut sessions: Vec<TimelineSession> = groups
        .into_iter()
        .map(|g| {
            let memory_count = g.memories.len();
            let project_name = g.project_id.as_deref()
                .and_then(|pid| project_names.get(pid).map(|s| s.to_string()));

            // Compute session stats
            let started = chrono::DateTime::parse_from_rfc3339(&g.started_at)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now());
            let ended = chrono::DateTime::parse_from_rfc3339(&g.ended_at)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now());
            let duration_secs = (ended - started).num_seconds().max(0);

            let mut type_counts = std::collections::HashMap::new();
            let mut source_counts = std::collections::HashMap::new();
            let mut total_strength = 0.0;
            for m in &g.memories {
                *type_counts.entry(m.memory_type.to_string()).or_insert(0) += 1;
                *source_counts.entry(m.source.to_string()).or_insert(0) += 1;
                total_strength += m.strength;
            }
            let avg_strength = if memory_count > 0 {
                total_strength / memory_count as f64
            } else {
                1.0
            };

            TimelineSession {
                session_id: g.session_id,
                project_id: g.project_id,
                project_name,
                started_at: g.started_at,
                ended_at: g.ended_at,
                memory_count,
                memories: g.memories,
                duration_secs,
                type_counts,
                source_counts,
                avg_strength,
            }
        })
        .collect();

    // Apply memory type filter
    if let Some(mt) = &q.memory_type {
        let mt_str = mt.as_str();
        for session in &mut sessions {
            session.memories.retain(|m| m.memory_type.to_string() == mt_str);
            session.memory_count = session.memories.len();
        }
        sessions.retain(|s| !s.memories.is_empty());
    }

    // Apply source filter
    if let Some(src) = &q.source {
        let src_str = src.as_str();
        for session in &mut sessions {
            session.memories.retain(|m| m.source.to_string() == src_str);
            session.memory_count = session.memories.len();
        }
        sessions.retain(|s| !s.memories.is_empty());
    }

    Ok(Json(TimelineResponse { sessions, total }))
}

// ---------------------------------------------------------------------------
// /api/projects, /api/stats
// ---------------------------------------------------------------------------

pub async fn list_projects(State(state): State<AppState>) -> ApiResult<Value> {
    let store = state.lock_store()?;
    let projects = store.list_projects()?;
    Ok(Json(json!({ "projects": projects })))
}

pub async fn stats(State(state): State<AppState>) -> ApiResult<Value> {
    let store = state.lock_store()?;
    let s = store.stats()?;
    let by_type: serde_json::Map<String, Value> =
        s.by_type.into_iter().map(|(t, n)| (t, json!(n))).collect();
    let by_tier: serde_json::Map<String, Value> =
        s.by_tier.into_iter().map(|(t, n)| (t, json!(n))).collect();
    Ok(Json(json!({
        "memory_count": s.memory_count,
        "edge_count": s.edge_count,
        "project_count": s.project_count,
        "pending_jobs": s.pending_jobs,
        "by_type": by_type,
        "by_tier": by_tier,
        "last_consolidation_at": s.last_consolidation_at,
    })))
}

// ---------------------------------------------------------------------------
// /api/settings
// ---------------------------------------------------------------------------

/// Dotted paths writable via PATCH. Secrets are never settable over HTTP.
const MUTABLE_SETTINGS: &[&str] = &[
    "memory.edges.similarity_threshold",
    "memory.edges.temporal_window_secs",
    "context.max_tokens",
    "enrichment.enabled",
    "llm.enabled",
    "llm.base_url",
    "llm.model",
    "dashboard.port",
    "dashboard.host",
];

fn sanitized_settings(config: &poneglyph_core::config::Config) -> Result<Value, ApiError> {
    let mut v = serde_json::to_value(config).map_err(ApiError::internal)?;
    let token_set = config.dashboard.token.as_deref().is_some_and(|t| !t.trim().is_empty());
    let key_set = config.llm.api_key.as_deref().is_some_and(|k| !k.trim().is_empty());
    if let Some(dashboard) = v.get_mut("dashboard").and_then(Value::as_object_mut) {
        dashboard.remove("token");
        dashboard.insert("token_set".into(), json!(token_set));
    }
    if let Some(llm) = v.get_mut("llm").and_then(Value::as_object_mut) {
        llm.remove("api_key");
        llm.insert("api_key_set".into(), json!(key_set));
    }
    Ok(v)
}

pub async fn get_settings(State(state): State<AppState>) -> ApiResult<Value> {
    Ok(Json(sanitized_settings(&state.config)?))
}

/// Flatten `{"graph": {"similarity_threshold": 0.9}}` into dotted paths.
fn flatten(prefix: &str, v: &Value, out: &mut Vec<(String, Value)>) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                let path = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                flatten(&path, child, out);
            }
        }
        _ => out.push((prefix.to_string(), v.clone())),
    }
}

pub async fn patch_settings(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<Value>,
) -> ApiResult<Value> {
    let mut updates = Vec::new();
    flatten("", &body, &mut updates);
    if updates.is_empty() {
        return Err(ApiError::bad_request("empty settings patch"));
    }
    for (path, _) in &updates {
        if !MUTABLE_SETTINGS.contains(&path.as_str()) {
            return Err(ApiError::bad_request(format!("setting not mutable via API: {path}")));
        }
    }

    // Merge into the *current* config, validate by round-trip, persist as TOML.
    let mut merged = serde_json::to_value(&*state.config).map_err(ApiError::internal)?;
    for (path, val) in &updates {
        let mut cursor = &mut merged;
        let parts: Vec<&str> = path.split('.').collect();
        for part in &parts[..parts.len() - 1] {
            cursor = cursor
                .get_mut(*part)
                .ok_or_else(|| ApiError::internal(format!("config section missing: {part}")))?;
        }
        let leaf = parts.last().unwrap();
        cursor
            .as_object_mut()
            .ok_or_else(|| ApiError::internal("config section is not an object"))?
            .insert((*leaf).to_string(), val.clone());
    }

    let new_config: poneglyph_core::config::Config = serde_json::from_value(merged)
        .map_err(|e| ApiError::bad_request(format!("invalid settings value: {e}")))?;

    let config_path = poneglyph_core::config::Config::default_config_path();
    if let Some(dir) = config_path.parent() {
        std::fs::create_dir_all(dir).map_err(ApiError::internal)?;
    }
    let toml_str = toml::to_string_pretty(&new_config).map_err(ApiError::internal)?;
    std::fs::write(&config_path, toml_str).map_err(ApiError::internal)?;

    Ok(Json(json!({
        "settings": sanitized_settings(&new_config)?,
        "restart_required": true,
    })))
}

// ---------------------------------------------------------------------------
// /api/session-summary
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct SessionSummaryQuery {
    pub project_path: Option<String>,
}

pub async fn session_summary(
    State(state): State<AppState>,
    Query(q): Query<SessionSummaryQuery>,
) -> ApiResult<Value> {
    let store = state.store.lock().map_err(ApiError::internal)?;
    let project_id = match &q.project_path {
        Some(path) => project_id_for(&store, path)?,
        None => None,
    };

    let (memories, _) = store.list_memories(project_id.as_deref(), Some("semantic"), 50, 0)
        .map_err(ApiError::internal)?;

    let summary = memories.iter().find(|m| {
        m.metadata.as_ref()
            .and_then(|meta| meta.get("tags"))
            .and_then(|tags| tags.as_array())
            .map(|arr| arr.iter().any(|t| t.as_str() == Some("session-summary")))
            .unwrap_or(false)
    });

    match summary {
        Some(mem) => Ok(Json(json!({
            "content": mem.content,
            "created_at": mem.created_at,
            "id": mem.id,
        }))),
        None => Ok(Json(json!(null))),
    }
}

// ---------------------------------------------------------------------------
// /healthz
// ---------------------------------------------------------------------------

pub async fn healthz() -> (StatusCode, Json<Value>) {
    (StatusCode::OK, Json(json!({ "ok": true })))
}

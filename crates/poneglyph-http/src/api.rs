//! Viewer API endpoints (PRD §8.8).
//!
//! Handlers follow the MCP discipline: embed before locking the store,
//! never await under the mutex, enqueue edge work instead of computing it.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use poneglyph_core::enrich;
use poneglyph_core::model::{Edge, Memory};
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
    Ok(Json(MemoryDetail { memory, edges }))
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
    let embedding = state.embed_or_none(&body.new_content).await?;

    let updated = {
        let store = state.lock_store()?;
        let updated = store.update_memory(&id, &body.new_content)?;
        if updated {
            store.index_fts(&id, &body.new_content)?;
            if let Some(vec) = &embedding {
                store.index_embedding(&id, vec)?;
            }
            enrich::enqueue_compute_edges(&store, &id)?;
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
    let query_vec = state.embed_or_none(&q.q).await?;

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

#[derive(Deserialize)]
pub struct GraphQuery {
    pub focus: Option<String>,
    pub depth: Option<u32>,
    pub limit: Option<usize>,
}

#[derive(Serialize)]
pub struct GraphResponse {
    pub nodes: Vec<Memory>,
    pub edges: Vec<Edge>,
}

pub async fn graph(
    State(state): State<AppState>,
    Query(q): Query<GraphQuery>,
) -> ApiResult<GraphResponse> {
    let depth = q.depth.unwrap_or(1).clamp(1, 5);
    let limit = q.limit.unwrap_or(500).clamp(1, 2000);

    let store = state.lock_store()?;
    let (nodes, edges) = match q.focus.as_deref() {
        Some(focus) => {
            if store.get_memory(focus)?.is_none() {
                return Err(ApiError::not_found(format!("memory not found: {focus}")));
            }
            store.graph_neighborhood(focus, depth, limit)?
        }
        None => store.graph_sample(limit)?,
    };
    Ok(Json(GraphResponse { nodes, edges }))
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
    Ok(Json(json!({
        "memory_count": s.memory_count,
        "edge_count": s.edge_count,
        "project_count": s.project_count,
        "pending_jobs": s.pending_jobs,
    })))
}

// ---------------------------------------------------------------------------
// /api/settings
// ---------------------------------------------------------------------------

/// Dotted paths writable via PATCH. Secrets are never settable over HTTP.
const MUTABLE_SETTINGS: &[&str] = &[
    "graph.similarity_threshold",
    "graph.temporal_window_secs",
    "context.max_tokens",
    "enrichment.enabled",
    "llm.enabled",
    "llm.endpoint",
    "llm.model",
    "server.http_port",
    "server.bind_addr",
];

fn sanitized_settings(config: &poneglyph_core::config::Config) -> Result<Value, ApiError> {
    let mut v = serde_json::to_value(config).map_err(ApiError::internal)?;
    let token_set = config.server.api_token.as_deref().is_some_and(|t| !t.trim().is_empty());
    let key_set = config.llm.api_key.as_deref().is_some_and(|k| !k.trim().is_empty());
    if let Some(server) = v.get_mut("server").and_then(Value::as_object_mut) {
        server.remove("api_token");
        server.insert("api_token_set".into(), json!(token_set));
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
// /healthz
// ---------------------------------------------------------------------------

pub async fn healthz() -> (StatusCode, Json<Value>) {
    (StatusCode::OK, Json(json!({ "ok": true })))
}

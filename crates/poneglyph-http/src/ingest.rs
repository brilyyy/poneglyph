//! Passive capture endpoint (PRD §8.7, event schema §10.2).
//!
//! Hook adapters POST session events here; each event becomes a
//! `source = passive` memory with the project attached and edge work
//! enqueued. The server never dedupes — event selection is the hook's job.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use poneglyph_core::enrich;
use poneglyph_core::model::{MemoryType, Source};

use crate::error::{ApiError, ApiJson};
use crate::state::AppState;

/// Passive memories rank below curated ones.
const PASSIVE_IMPORTANCE: f64 = 0.3;
/// Cheap garbage guard; hooks already truncate, this is the backstop.
const MAX_CONTENT_BYTES: usize = 100 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestEventKind {
    ToolUse,
    UserMessage,
    AssistantMessage,
    FileEdit,
    Terminal,
}

impl IngestEventKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::ToolUse => "tool_use",
            Self::UserMessage => "user_message",
            Self::AssistantMessage => "assistant_message",
            Self::FileEdit => "file_edit",
            Self::Terminal => "terminal",
        }
    }

    fn memory_type(&self) -> MemoryType {
        match self {
            Self::ToolUse | Self::FileEdit | Self::Terminal => MemoryType::CodeContext,
            Self::UserMessage | Self::AssistantMessage => MemoryType::Episodic,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct IngestEvent {
    pub event: IngestEventKind,
    /// "claude-code" | "opencode" | "custom"
    pub client: String,
    pub project_path: Option<String>,
    pub content: String,
    /// Tool name for tool_use events.
    pub tool: Option<String>,
    pub metadata: Option<Value>,
    /// ISO-8601; server fills with now() if absent.
    pub timestamp: Option<String>,
}

pub async fn ingest(
    State(state): State<AppState>,
    ApiJson(mut ev): ApiJson<IngestEvent>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    if ev.content.trim().is_empty() {
        return Err(ApiError::bad_request("content must be non-empty"));
    }
    if ev.content.len() > MAX_CONTENT_BYTES {
        return Err(ApiError(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("content exceeds {MAX_CONTENT_BYTES} bytes"),
        ));
    }

    // Never index content naming an excluded path (.env, *.pem, secrets/**).
    let exclude_matcher = poneglyph_core::privacy::build_exclude_matcher(&state.config.privacy.exclude_paths);
    if poneglyph_core::privacy::content_references_excluded_path(&ev.content, &exclude_matcher) {
        return Ok((StatusCode::ACCEPTED, Json(json!({ "skipped": "excluded_path" }))));
    }
    ev.content = poneglyph_core::privacy::redact_content(&ev.content, &state.config.privacy);
    if ev.content.trim().is_empty() {
        return Ok((StatusCode::ACCEPTED, Json(json!({ "skipped": "redacted_empty" }))));
    }

    // Tags double as the AC marker: memory is "tagged with tool name and project".
    let mut tags = vec![ev.client.clone()];
    if let Some(tool) = &ev.tool {
        tags.push(tool.clone());
    }
    let mut metadata = json!({
        "tags": tags,
        "event": ev.event.as_str(),
        "client": ev.client,
        "timestamp": ev.timestamp.unwrap_or_else(|| chrono_now()),
    });
    if let Some(tool) = &ev.tool {
        metadata["tool"] = json!(tool);
    }
    if let Some(extra) = &ev.metadata {
        metadata["extra"] = extra.clone();
    }
    // Canonical session key: hoist ev.metadata.session_id to top-level.
    // Legacy extra.session_id is handled by the SQL query in list_sessions.
    if let Some(sid) = ev.metadata.as_ref().and_then(|m| m.get("session_id")).and_then(Value::as_str) {
        metadata["session_id"] = json!(sid);
    }

    // Embed before taking the lock (no await under the mutex).
    let embedding = state.embed_or_none(&ev.content).await?;

    let id = {
        let store = state.lock_store()?;

        let project_id = match ev.project_path.as_deref() {
            Some(path) => Some(poneglyph_core::project::detect_project(&store, path)?.id),
            None => None,
        };

        let mem = store.create_memory(
            &ev.content,
            ev.event.memory_type(),
            PASSIVE_IMPORTANCE,
            Source::Passive,
            project_id.as_deref(),
            Some(&metadata),
        )?;

        store.index_fts(&mem.id, &ev.content)?;
        if let Some(vec) = &embedding {
            store.index_embedding(&mem.id, vec)?;
        }
        enrich::enqueue_compute_edges(&store, &mem.id)?;

        // Passive capture is high-volume; only summarize is worth LLM time.
        if state.config.enrichment.enabled && state.config.llm.enabled {
            store.create_job(poneglyph_core::model::JobType::Summarize, &mem.id)?;
        }

        // Compression is orthogonal to enrichment.
        if state.config.memory.compression_enabled {
            enrich::enqueue_compression(&store, &mem.id, state.config.memory.compression_mode)?;
        }

        mem.id
    };

    if let Some(h) = &state.enrich {
        h.notify();
    }

    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

fn chrono_now() -> String {
    chrono::Utc::now().to_rfc3339()
}

use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use serde::Serialize;
use std::path::Path;
use std::str::FromStr;
use tracing::info;
use uuid::Uuid;
use crate::model::*;

pub const SCHEMA_VERSION: i64 = 4;

/// Max ids per SQL `IN (...)` list — stays under SQLite's default 999-param limit.
const SQL_IN_CHUNK: usize = 900;

// ---------------------------------------------------------------------------
// Extension loading — call once per process, before any Connection
// ---------------------------------------------------------------------------

static INIT: std::sync::Once = std::sync::Once::new();

pub fn init_sqlite_vec() {
    INIT.call_once(|| {
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
    });
}

// ---------------------------------------------------------------------------
// DDL
// ---------------------------------------------------------------------------

const DDL_V1: &str = r#"
CREATE TABLE IF NOT EXISTS memories (
    id            TEXT PRIMARY KEY,
    content       TEXT NOT NULL,
    memory_type   TEXT NOT NULL,
    importance    REAL NOT NULL DEFAULT 0.5,
    project_id    TEXT,
    source        TEXT NOT NULL,
    metadata      TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    accessed_at   TEXT,
    access_count  INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE SET NULL
);
CREATE INDEX IF NOT EXISTS idx_mem_project ON memories(project_id);
CREATE INDEX IF NOT EXISTS idx_mem_type    ON memories(memory_type);
CREATE INDEX IF NOT EXISTS idx_mem_created ON memories(created_at);

CREATE VIRTUAL TABLE IF NOT EXISTS vec_memories USING vec0(
    memory_id TEXT PRIMARY KEY,
    embedding FLOAT[384]
);

CREATE VIRTUAL TABLE IF NOT EXISTS fts_memories USING fts5(
    memory_id UNINDEXED,
    content,
    tokenize = 'porter unicode61'
);

CREATE TABLE IF NOT EXISTS edges (
    id          TEXT PRIMARY KEY,
    src_id      TEXT NOT NULL,
    dst_id      TEXT NOT NULL,
    edge_type   TEXT NOT NULL,
    label       TEXT,
    weight      REAL NOT NULL DEFAULT 1.0,
    created_at  TEXT NOT NULL,
    FOREIGN KEY (src_id) REFERENCES memories(id) ON DELETE CASCADE,
    FOREIGN KEY (dst_id) REFERENCES memories(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_edge_src ON edges(src_id);
CREATE INDEX IF NOT EXISTS idx_edge_dst ON edges(dst_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_edge_unique ON edges(src_id, dst_id, edge_type, COALESCE(label,''));

CREATE TABLE IF NOT EXISTS projects (
    id           TEXT PRIMARY KEY,
    path         TEXT NOT NULL,
    git_remote   TEXT,
    name         TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    last_seen_at TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_project_path ON projects(path);

CREATE TABLE IF NOT EXISTS jobs (
    id          TEXT PRIMARY KEY,
    job_type    TEXT NOT NULL,
    memory_id   TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'pending',
    attempts    INTEGER NOT NULL DEFAULT 0,
    last_error  TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    FOREIGN KEY (memory_id) REFERENCES memories(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_jobs_status ON jobs(status);

CREATE TABLE IF NOT EXISTS schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
"#;

const DDL_V2: &str = r#"
-- Schema decoy columns
ALTER TABLE memories ADD COLUMN is_decoy    INTEGER DEFAULT 0;
ALTER TABLE memories ADD COLUMN tier        TEXT DEFAULT 'hot';
ALTER TABLE memories ADD COLUMN strength    REAL DEFAULT 1.0;
ALTER TABLE memories ADD COLUMN cold_path   TEXT;

-- Decoy → child relationships (schema consolidation)
CREATE TABLE IF NOT EXISTS decoy_children (
    decoy_id    TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    child_id    TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    PRIMARY KEY (decoy_id, child_id)
);
CREATE INDEX IF NOT EXISTS idx_dc_child ON decoy_children(child_id);

CREATE INDEX IF NOT EXISTS idx_mem_tier     ON memories(tier);
CREATE INDEX IF NOT EXISTS idx_mem_strength ON memories(strength);
CREATE INDEX IF NOT EXISTS idx_mem_decoy    ON memories(is_decoy);
"#;

// Code knowledge graph (Tree-sitter). Distinct from the `edges` table above
// (memory-linkage similarity/temporal/tag edges).
const DDL_V3: &str = r#"
CREATE TABLE IF NOT EXISTS cg_files (
    path          TEXT PRIMARY KEY,
    language      TEXT NOT NULL,
    content_hash  TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS cg_nodes (
    id          TEXT PRIMARY KEY,
    file_path   TEXT NOT NULL,
    kind        TEXT NOT NULL,
    name        TEXT NOT NULL,
    start_line  INTEGER NOT NULL,
    end_line    INTEGER NOT NULL,
    FOREIGN KEY (file_path) REFERENCES cg_files(path) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_cg_nodes_file ON cg_nodes(file_path);
CREATE INDEX IF NOT EXISTS idx_cg_nodes_name ON cg_nodes(name);

CREATE TABLE IF NOT EXISTS cg_edges (
    src_id  TEXT NOT NULL,
    dst_id  TEXT NOT NULL,
    kind    TEXT NOT NULL,
    PRIMARY KEY (src_id, dst_id, kind),
    FOREIGN KEY (src_id) REFERENCES cg_nodes(id) ON DELETE CASCADE,
    FOREIGN KEY (dst_id) REFERENCES cg_nodes(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_cg_edges_src ON cg_edges(src_id);
CREATE INDEX IF NOT EXISTS idx_cg_edges_dst ON cg_edges(dst_id);
"#;

// Compression cache. `compressed_content` is never read by recall/FTS/vector
// search — only by context-injection (project::get_project_context), which
// falls back to `content` when absent. `content` itself is never overwritten.
const DDL_V4: &str = r#"
ALTER TABLE memories ADD COLUMN compressed_content TEXT;
ALTER TABLE memories ADD COLUMN compression_mode    TEXT;
"#;

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

pub struct Store {
    pub conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        init_sqlite_vec();

        let conn = Connection::open(path)
            .with_context(|| format!("failed to open db: {}", path.display()))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .context("failed to set pragmas")?;

        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        init_sqlite_vec();
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    // -----------------------------------------------------------------------
    // Migrations
    // -----------------------------------------------------------------------

    fn migrate(&mut self) -> Result<()> {
        let version: i64 = self
            .conn
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM schema_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if version < 1 {
            self.conn.execute_batch(DDL_V1).context("DDL v1 failed")?;
            self.conn.execute(
                "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('schema_version', '1')",
                [],
            )?;
            info!(version = 1, "schema migrated");
        }

        if version < 2 {
            self.conn.execute_batch(DDL_V2).context("DDL v2 failed")?;
            self.conn.execute(
                "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('schema_version', '2')",
                [],
            )?;
            info!(version = 2, "schema migrated");
        }

        if version < 3 {
            self.conn.execute_batch(DDL_V3).context("DDL v3 failed")?;
            self.conn.execute(
                "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('schema_version', '3')",
                [],
            )?;
            info!(version = 3, "schema migrated");
        }

        if version < 4 {
            self.conn.execute_batch(DDL_V4).context("DDL v4 failed")?;
            self.conn.execute(
                "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('schema_version', '4')",
                [],
            )?;
            info!(version = 4, "schema migrated");
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Projects
    // -----------------------------------------------------------------------

    pub fn upsert_project(&self, path: &str, name: &str, git_remote: Option<&str>) -> Result<Project> {
        let now = chrono::Utc::now().to_rfc3339();

        // Try update first
        let updated = self.conn.execute(
            "UPDATE projects SET last_seen_at = ?1, name = ?2, git_remote = ?3 WHERE path = ?4",
            params![now, name, git_remote, path],
        )?;

        if updated > 0 {
            let project = self.conn.query_row(
                "SELECT id, path, git_remote, name, created_at, last_seen_at FROM projects WHERE path = ?1",
                params![path],
                |row| row_to_project(row),
            )?;
            return Ok(project);
        }

        // Insert new
        let id = Uuid::now_v7().to_string();
        self.conn.execute(
            "INSERT INTO projects (id, path, git_remote, name, created_at, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, path, git_remote, name, now, now],
        )?;

        Ok(Project {
            id,
            path: path.to_string(),
            git_remote: git_remote.map(String::from),
            name: name.to_string(),
            created_at: chrono::Utc::now(),
            last_seen_at: chrono::Utc::now(),
        })
    }

    pub fn get_project(&self, path: &str) -> Result<Option<Project>> {
        let result = self.conn.query_row(
            "SELECT id, path, git_remote, name, created_at, last_seen_at FROM projects WHERE path = ?1",
            params![path],
            |row| row_to_project(row),
        );
        match result {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Stable identity across clones (PRD §8.10 AC2).
    pub fn get_project_by_remote(&self, git_remote: &str) -> Result<Option<Project>> {
        let result = self.conn.query_row(
            "SELECT id, path, git_remote, name, created_at, last_seen_at FROM projects WHERE git_remote = ?1",
            params![git_remote],
            |row| row_to_project(row),
        );
        match result {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Bump `last_seen_at` without touching path/name/remote.
    pub fn touch_project(&self, id: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE projects SET last_seen_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }

    /// Backfill `git_remote` when it was NULL (path-only project later opened
    /// in a git checkout).
    pub fn set_project_remote(&self, id: &str, git_remote: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE projects SET git_remote = ?1 WHERE id = ?2 AND git_remote IS NULL",
            params![git_remote, id],
        )?;
        Ok(())
    }

    pub fn list_projects(&self) -> Result<Vec<Project>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, git_remote, name, created_at, last_seen_at FROM projects ORDER BY last_seen_at DESC",
        )?;
        let projects = stmt.query_map([], |row| row_to_project(row))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(projects)
    }

    // -----------------------------------------------------------------------
    // Memories — CRUD
    // -----------------------------------------------------------------------

    pub fn create_memory(
        &self,
        content: &str,
        memory_type: MemoryType,
        importance: f64,
        source: Source,
        project_id: Option<&str>,
        metadata: Option<&serde_json::Value>,
    ) -> Result<Memory> {
        let id = Uuid::now_v7().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let meta_str = metadata.map(|v| serde_json::to_string(v)).transpose()?;

        self.conn.execute(
            "INSERT INTO memories (id, content, memory_type, importance, project_id, source, metadata, created_at, updated_at, access_count, is_decoy, tier, strength, cold_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, 0, 'hot', 1.0, NULL)",
            params![id, content, memory_type.to_string(), importance, project_id, source.to_string(), meta_str, now, now],
        )?;

        Ok(Memory {
            id,
            content: content.to_string(),
            memory_type,
            importance,
            project_id: project_id.map(String::from),
            source,
            metadata: metadata.cloned(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            accessed_at: None,
            access_count: 0,
            is_decoy: false,
            tier: Tier::Hot,
            strength: 1.0,
            cold_path: None,
        })
    }

    pub fn get_memory(&self, id: &str) -> Result<Option<Memory>> {
        let result = self.conn.query_row(
            "SELECT id, content, memory_type, importance, project_id, source, metadata, created_at, updated_at, accessed_at, access_count,
                    is_decoy, tier, strength, cold_path
             FROM memories WHERE id = ?1",
            params![id],
            |row| row_to_memory(row),
        );
        match result {
            Ok(m) => Ok(Some(m)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn list_memories(
        &self,
        project_id: Option<&str>,
        memory_type: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<Memory>, i64)> {
        let mut conditions = Vec::new();
        let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(pid) = project_id {
            conditions.push("project_id = ?".to_string());
            values.push(Box::new(pid.to_string()));
        }
        if let Some(mt) = memory_type {
            conditions.push("memory_type = ?".to_string());
            values.push(Box::new(mt.to_string()));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let count_sql = format!("SELECT COUNT(*) FROM memories {where_clause}");
        let total: i64 = {
            let mut stmt = self.conn.prepare(&count_sql)?;
            let params_refs: Vec<&dyn rusqlite::types::ToSql> = values.iter().map(|v| v.as_ref()).collect();
            stmt.query_row(params_refs.as_slice(), |row| row.get(0))?
        };

        let query_sql = format!(
            "SELECT id, content, memory_type, importance, project_id, source, metadata, created_at, updated_at, accessed_at, access_count,
                    is_decoy, tier, strength, cold_path
             FROM memories {where_clause} ORDER BY created_at DESC LIMIT ? OFFSET ?"
        );
        values.push(Box::new(limit as i64));
        values.push(Box::new(offset as i64));

        let mut stmt = self.conn.prepare(&query_sql)?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> = values.iter().map(|v| v.as_ref()).collect();
        let memories = stmt.query_map(params_refs.as_slice(), |row| row_to_memory(row))?
            .collect::<rusqlite::Result<_>>()?;

        Ok((memories, total))
    }

    /// Shallow-merge a JSON object into `metadata` (NULL → `{}`); arrays
    /// under `tags`/`entities` are union-deduped so enrichment never clobbers
    /// caller-provided tags. Bumps `updated_at`.
    pub fn merge_metadata(&self, id: &str, patch: &serde_json::Value) -> Result<bool> {
        let Some(patch_obj) = patch.as_object() else {
            anyhow::bail!("metadata patch must be a JSON object");
        };

        let current: Option<Option<String>> = self
            .conn
            .query_row("SELECT metadata FROM memories WHERE id = ?1", params![id], |r| r.get(0))
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                e => Err(e),
            })?;
        let Some(current) = current else { return Ok(false) };

        let mut merged: serde_json::Value = current
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        let obj = merged.as_object_mut().expect("metadata root is an object");

        for (key, val) in patch_obj {
            let is_union_key = key == "tags" || key == "entities";
            match (is_union_key, obj.get(key).and_then(|v| v.as_array()), val.as_array()) {
                (true, Some(existing), Some(incoming)) => {
                    let mut union = existing.clone();
                    for item in incoming {
                        if !union.contains(item) {
                            union.push(item.clone());
                        }
                    }
                    obj.insert(key.clone(), serde_json::Value::Array(union));
                }
                _ => {
                    obj.insert(key.clone(), val.clone());
                }
            }
        }

        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE memories SET metadata = ?1, updated_at = ?2 WHERE id = ?3",
            params![serde_json::to_string(&merged)?, now, id],
        )?;
        Ok(true)
    }

    /// Set importance (clamped 0..=1), bump `updated_at`.
    pub fn set_importance(&self, id: &str, importance: f64) -> Result<bool> {
        let now = chrono::Utc::now().to_rfc3339();
        let updated = self.conn.execute(
            "UPDATE memories SET importance = ?1, updated_at = ?2 WHERE id = ?3",
            params![importance.clamp(0.0, 1.0), now, id],
        )?;
        Ok(updated > 0)
    }

    /// Cache a token-reduced rewrite for context injection. Never touches
    /// `content` — recall/FTS/vector search must keep reading the original.
    pub fn set_compressed_content(&self, id: &str, text: &str, mode: &str) -> Result<bool> {
        let updated = self.conn.execute(
            "UPDATE memories SET compressed_content = ?1, compression_mode = ?2 WHERE id = ?3",
            params![text, mode, id],
        )?;
        Ok(updated > 0)
    }

    /// Drop a stale compressed rewrite — call when `content` changes so
    /// context-injection doesn't serve text for the previous edit.
    pub fn clear_compressed_content(&self, id: &str) -> Result<bool> {
        let updated = self.conn.execute(
            "UPDATE memories SET compressed_content = NULL, compression_mode = NULL WHERE id = ?1",
            params![id],
        )?;
        Ok(updated > 0)
    }

    pub fn get_compressed_content(&self, id: &str) -> Result<Option<(String, String)>> {
        self.conn
            .query_row(
                "SELECT compressed_content, compression_mode FROM memories \
                 WHERE id = ?1 AND compressed_content IS NOT NULL",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map(Some)
            .or_else(|e| if e == rusqlite::Error::QueryReturnedNoRows { Ok(None) } else { Err(e.into()) })
    }

    pub fn update_memory(&self, id: &str, new_content: &str) -> Result<bool> {
        let now = chrono::Utc::now().to_rfc3339();
        let updated = self.conn.execute(
            "UPDATE memories SET content = ?1, updated_at = ?2 WHERE id = ?3",
            params![new_content, now, id],
        )?;
        Ok(updated > 0)
    }

    pub fn delete_memory(&self, id: &str) -> Result<bool> {
        // Delete vec entry
        self.conn.execute("DELETE FROM vec_memories WHERE memory_id = ?1", params![id])?;
        // Delete fts entry
        self.conn.execute("DELETE FROM fts_memories WHERE memory_id = ?1", params![id])?;
        // Delete memory (edges cascade via FK)
        let deleted = self.conn.execute("DELETE FROM memories WHERE id = ?1", params![id])?;
        Ok(deleted > 0)
    }

    // -----------------------------------------------------------------------
    // Decoy / Schema operations
    // -----------------------------------------------------------------------

    /// Create a schema decoy memory (is_decoy=1, tier=hot).
    pub fn create_decoy(
        &self,
        content: &str,
        importance: f64,
        project_id: Option<&str>,
        metadata: Option<&serde_json::Value>,
    ) -> Result<Memory> {
        let id = Uuid::now_v7().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let meta_str = metadata.map(|v| serde_json::to_string(v)).transpose()?;

        self.conn.execute(
            "INSERT INTO memories (id, content, memory_type, importance, project_id, source, metadata, created_at, updated_at, access_count, is_decoy, tier, strength, cold_path)
             VALUES (?1, ?2, 'semantic', ?3, ?4, 'passive', ?5, ?6, ?7, 0, 1, 'hot', 1.0, NULL)",
            params![id, content, importance, project_id, meta_str, now, now],
        )?;

        Ok(Memory {
            id,
            content: content.to_string(),
            memory_type: MemoryType::Semantic,
            importance,
            project_id: project_id.map(String::from),
            source: Source::Passive,
            metadata: metadata.cloned(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            accessed_at: None,
            access_count: 0,
            is_decoy: true,
            tier: Tier::Hot,
            strength: 1.0,
            cold_path: None,
        })
    }

    /// Link a child memory to a decoy.
    pub fn link_decoy_child(&self, decoy_id: &str, child_id: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO decoy_children (decoy_id, child_id) VALUES (?1, ?2)",
            params![decoy_id, child_id],
        )?;
        Ok(())
    }

    /// Get all children of a decoy.
    pub fn get_decoy_children(&self, decoy_id: &str) -> Result<Vec<Memory>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.id, m.content, m.memory_type, m.importance, m.project_id, m.source, m.metadata,
                    m.created_at, m.updated_at, m.accessed_at, m.access_count,
                    m.is_decoy, m.tier, m.strength, m.cold_path
             FROM memories m
             INNER JOIN decoy_children dc ON m.id = dc.child_id
             WHERE dc.decoy_id = ?1",
        )?;
        let memories = stmt.query_map(params![decoy_id], |row| row_to_memory(row))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(memories)
    }

    /// Get the decoy that owns a given child (if any).
    pub fn get_child_decoy(&self, child_id: &str) -> Result<Option<Memory>> {
        let result = self.conn.query_row(
            "SELECT m.id, m.content, m.memory_type, m.importance, m.project_id, m.source, m.metadata,
                    m.created_at, m.updated_at, m.accessed_at, m.access_count,
                    m.is_decoy, m.tier, m.strength, m.cold_path
             FROM memories m
             INNER JOIN decoy_children dc ON m.id = dc.decoy_id
             WHERE dc.child_id = ?1",
            params![child_id],
            |row| row_to_memory(row),
        );
        match result {
            Ok(m) => Ok(Some(m)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Mark a memory as consolidated (child of a decoy). Sets tier=warm.
    pub fn mark_consolidated(&self, memory_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE memories SET tier = 'warm', updated_at = ?1 WHERE id = ?2",
            params![chrono::Utc::now().to_rfc3339(), memory_id],
        )?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Strength / Decay operations
    // -----------------------------------------------------------------------

    /// Compute Ebbinghaus strength for a memory based on access pattern.
    /// strength(t) = e^(-t / stability), stability = 1 + ln(1 + access_count) * recency_boost
    pub fn compute_strength(&self, memory_id: &str) -> Result<f64> {
        let row = self.conn.query_row(
            "SELECT created_at, accessed_at, access_count FROM memories WHERE id = ?1",
            params![memory_id],
            |r| {
                let created: String = r.get(0)?;
                let accessed: Option<String> = r.get(1)?;
                let access_count: i64 = r.get(2)?;
                Ok((created, accessed, access_count))
            },
        );

        let (created_str, accessed_str, access_count) = match row {
            Ok(r) => r,
            Err(_) => return Ok(1.0),
        };

        let created_at = chrono::DateTime::parse_from_rfc3339(&created_str)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now());

        let last_access = accessed_str
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or(created_at);

        let now = chrono::Utc::now();
        let age_days = (now - created_at).num_seconds() as f64 / 86400.0;
        let days_since_access = (now - last_access).num_seconds() as f64 / 86400.0;

        // Stability increases with access count and recency of access
        let recency_boost = 1.0 / (1.0 + days_since_access / 7.0);
        let stability = 1.0 + (1.0 + access_count as f64).ln() * recency_boost;

        // Ebbinghaus forgetting curve
        let strength = (-age_days / stability).exp();

        Ok(strength.clamp(0.0, 1.0))
    }

    /// Reinforce a memory's strength on access (spaced repetition boost).
    pub fn reinforce_strength(&self, memory_id: &str) -> Result<()> {
        let current_strength = self.compute_strength(memory_id)?;
        // Weak memories get bigger boost (like SM-2)
        let boost = 0.5 * (1.0 - current_strength);
        let new_strength = (current_strength + boost).clamp(0.0, 1.0);

        self.conn.execute(
            "UPDATE memories SET strength = ?1, updated_at = ?2 WHERE id = ?3",
            params![new_strength, chrono::Utc::now().to_rfc3339(), memory_id],
        )?;
        Ok(())
    }

    /// Update strength for all memories (called by decay worker).
    pub fn update_all_strengths(&self) -> Result<i64> {
        let ids: Vec<String> = {
            let mut stmt = self.conn.prepare("SELECT id FROM memories WHERE is_decoy = 0")?;
            stmt.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };

        let mut updated = 0;
        for id in &ids {
            let strength = self.compute_strength(id)?;
            self.conn.execute(
                "UPDATE memories SET strength = ?1 WHERE id = ?2",
                params![strength, id],
            )?;
            updated += 1;
        }

        // Also compute decoy strength as max of children
        let decoy_ids: Vec<String> = {
            let mut stmt = self.conn.prepare("SELECT id FROM memories WHERE is_decoy = 1")?;
            stmt.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };

        for did in &decoy_ids {
            let max_strength: f64 = self.conn.query_row(
                "SELECT COALESCE(MAX(m.strength), 0.0)
                 FROM memories m
                 INNER JOIN decoy_children dc ON m.id = dc.child_id
                 WHERE dc.decoy_id = ?1",
                params![did],
                |r| r.get(0),
            )?;
            self.conn.execute(
                "UPDATE memories SET strength = ?1 WHERE id = ?2",
                params![max_strength, did],
            )?;
        }

        Ok(updated)
    }

    /// Get memories eligible for cold storage (low strength, old, not decoys).
    pub fn get_cold_candidates(&self, min_strength: f64, min_age_days: i64) -> Result<Vec<Memory>> {
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(min_age_days)).to_rfc3339();
        let mut stmt = self.conn.prepare(
            "SELECT id, content, memory_type, importance, project_id, source, metadata,
                    created_at, updated_at, accessed_at, access_count,
                    is_decoy, tier, strength, cold_path
             FROM memories
             WHERE is_decoy = 0
               AND tier != 'cold'
               AND strength < ?1
               AND created_at < ?2
               AND access_count < 3",
        )?;
        let memories = stmt.query_map(params![min_strength, cutoff], |row| row_to_memory(row))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(memories)
    }

    /// Move a memory to cold tier (update tier + cold_path, keep content for now).
    pub fn move_to_cold(&self, memory_id: &str, cold_path: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE memories SET tier = 'cold', cold_path = ?1, updated_at = ?2 WHERE id = ?3",
            params![cold_path, chrono::Utc::now().to_rfc3339(), memory_id],
        )?;
        Ok(())
    }

    /// Get memories eligible for consolidation (low strength, same project).
    pub fn get_consolidation_candidates(&self, project_id: &str, threshold: f64) -> Result<Vec<Memory>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content, memory_type, importance, project_id, source, metadata,
                    created_at, updated_at, accessed_at, access_count,
                    is_decoy, tier, strength, cold_path
             FROM memories
             WHERE project_id = ?1
               AND is_decoy = 0
               AND strength < ?2
               AND tier != 'cold'",
        )?;
        let memories = stmt.query_map(params![project_id, threshold], |row| row_to_memory(row))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(memories)
    }

    // -----------------------------------------------------------------------
    // Vec + FTS indexing
    // -----------------------------------------------------------------------

    pub fn index_embedding(&self, memory_id: &str, embedding: &[f32]) -> Result<()> {
        // Convert f32 slice to little-endian bytes for sqlite-vec
        let mut bytes = Vec::with_capacity(embedding.len() * 4);
        for &f in embedding {
            bytes.extend_from_slice(&f.to_le_bytes());
        }

        // Upsert vec entry
        self.conn.execute(
            "DELETE FROM vec_memories WHERE memory_id = ?1",
            params![memory_id],
        )?;
        self.conn.execute(
            "INSERT INTO vec_memories (memory_id, embedding) VALUES (?1, ?2)",
            params![memory_id, bytes],
        )?;

        Ok(())
    }

    pub fn index_fts(&self, memory_id: &str, content: &str) -> Result<()> {
        // FTS5 doesn't support UPDATE, so delete + insert
        self.conn.execute(
            "DELETE FROM fts_memories WHERE memory_id = ?1",
            params![memory_id],
        )?;
        self.conn.execute(
            "INSERT INTO fts_memories (memory_id, content) VALUES (?1, ?2)",
            params![memory_id, content],
        )?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Sessions / Timeline
    // -----------------------------------------------------------------------

    /// Fetch all memories ordered by `created_at ASC`, optionally filtered by
    /// project, then group them into sessions. Rows with a `session_id` in
    /// metadata (top-level or `extra.session_id`) are grouped by that key.
    /// Rows without are bucketed per project, splitting when the gap between
    /// consecutive rows exceeds `gap_secs`.
    ///
    /// Returns `(sessions_page, total_session_count)` — sessions sorted by
    /// `started_at DESC`, paginated by session index (not row count).
    pub fn list_sessions(
        &self,
        project_id: Option<&str>,
        gap_secs: i64,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<SessionGroup>, i64)> {
        let mut conditions = vec!["1=1".to_string()];
        let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(pid) = project_id {
            conditions.push("project_id = ?".to_string());
            values.push(Box::new(pid.to_string()));
        }

        let where_clause = conditions.join(" AND ");
        let sql = format!(
            "SELECT id, content, memory_type, importance, project_id, source, metadata,
                    created_at, updated_at, accessed_at, access_count,
                    is_decoy, tier, strength, cold_path,
                    COALESCE(json_extract(metadata, '$.session_id'), json_extract(metadata, '$.extra.session_id')) AS session_key
             FROM memories WHERE {where_clause} ORDER BY created_at ASC"
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> = values.iter().map(|v| v.as_ref()).collect();
        let rows: Vec<(Memory, Option<String>)> = stmt
            .query_map(params_refs.as_slice(), |row| {
                let mem = row_to_memory(row)?;
                let session_key: Option<String> = row.get(15)?;
                Ok((mem, session_key))
            })?
            .collect::<rusqlite::Result<_>>()?;

        // Group into sessions.
        let mut keyed: std::collections::HashMap<String, Vec<Memory>> = std::collections::HashMap::new();
        // Unkeyed rows grouped by project, in order.
        let mut unkeyed_order: Vec<(Option<String>, usize)> = Vec::new(); // (project_id, start_idx)
        let mut unkeyed_all: Vec<Memory> = Vec::new();
        // Track current project group.
        let mut current_unkeyed_project: Option<String> = None;
        let mut current_unkeyed_start: usize = 0;

        for (mem, session_key) in &rows {
            if let Some(key) = session_key {
                keyed.entry(key.clone()).or_default().push(mem.clone());
            } else {
                let pid = mem.project_id.clone();
                if current_unkeyed_project != pid {
                    if !unkeyed_all.is_empty() {
                        unkeyed_order.push((current_unkeyed_project, current_unkeyed_start));
                    }
                    current_unkeyed_start = unkeyed_all.len();
                    current_unkeyed_project = pid;
                }
                unkeyed_all.push(mem.clone());
            }
        }
        if !unkeyed_all.is_empty() {
            unkeyed_order.push((current_unkeyed_project, current_unkeyed_start));
        }

        let gap = chrono::Duration::seconds(gap_secs);
        let mut sessions: Vec<SessionGroup> = Vec::new();

        // Keyed sessions.
        for (sid, mems) in &keyed {
            let first = mems.first().unwrap();
            let last = mems.last().unwrap();
            sessions.push(SessionGroup {
                session_id: Some(sid.clone()),
                project_id: first.project_id.clone(),
                started_at: first.created_at.to_rfc3339(),
                ended_at: last.created_at.to_rfc3339(),
                memories: mems.clone(),
            });
        }

        // Unkeyed sessions: split by gap within each project group.
        for (_pid, start_idx) in &unkeyed_order {
            let end_idx = unkeyed_order
                .iter()
                .find(|(_, s)| s > start_idx)
                .map(|(_, s)| *s)
                .unwrap_or(unkeyed_all.len());
            let group = &unkeyed_all[*start_idx..end_idx];
            let mut current: Vec<Memory> = Vec::new();
            for mem in group {
                if let Some(prev) = current.last() {
                    if mem.created_at - prev.created_at > gap {
                        let first = current.first().unwrap();
                        let last_mem = current.last().unwrap();
                        sessions.push(SessionGroup {
                            session_id: None,
                            project_id: first.project_id.clone(),
                            started_at: first.created_at.to_rfc3339(),
                            ended_at: last_mem.created_at.to_rfc3339(),
                            memories: std::mem::take(&mut current),
                        });
                    }
                }
                current.push(mem.clone());
            }
            if !current.is_empty() {
                let first = current.first().unwrap();
                let last_mem = current.last().unwrap();
                sessions.push(SessionGroup {
                    session_id: None,
                    project_id: first.project_id.clone(),
                    started_at: first.created_at.to_rfc3339(),
                    ended_at: last_mem.created_at.to_rfc3339(),
                    memories: current,
                });
            }
        }

        // Sort by started_at DESC.
        sessions.sort_by(|a, b| b.started_at.cmp(&a.started_at));

        let total = sessions.len() as i64;
        let page = sessions.into_iter().skip(offset).take(limit).collect();
        Ok((page, total))
    }

    // -----------------------------------------------------------------------
    // Edges
    // -----------------------------------------------------------------------

    pub fn create_edge(
        &self,
        src_id: &str,
        dst_id: &str,
        edge_type: EdgeType,
        label: Option<&str>,
        weight: f64,
    ) -> Result<Option<Edge>> {
        let id = Uuid::now_v7().to_string();
        let now = chrono::Utc::now().to_rfc3339();

        // Use INSERT OR IGNORE to respect unique constraint
        self.conn.execute(
            "INSERT OR IGNORE INTO edges (id, src_id, dst_id, edge_type, label, weight, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, src_id, dst_id, edge_type.to_string(), label, weight, now],
        )?;

        let changes = self.conn.changes();

        if changes > 0 {
            Ok(Some(Edge {
                id,
                src_id: src_id.to_string(),
                dst_id: dst_id.to_string(),
                edge_type,
                label: label.map(String::from),
                weight,
                created_at: chrono::Utc::now(),
            }))
        } else {
            Ok(None) // Already existed
        }
    }

    pub fn get_edges_for_memory(&self, memory_id: &str) -> Result<Vec<Edge>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, src_id, dst_id, edge_type, label, weight, created_at
             FROM edges WHERE src_id = ?1 OR dst_id = ?1",
        )?;
        let edges = stmt.query_map(params![memory_id], |row| row_to_edge(row))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(edges)
    }

    /// Iterative BFS from `focus` up to `depth` hops, capped at `max_nodes`
    /// total nodes. Returns the visited memories plus the edges among them.
    /// Edges are traversed in both directions (graph is undirected for
    /// exploration purposes).
    pub fn graph_neighborhood(
        &self,
        focus: &str,
        depth: u32,
        max_nodes: usize,
    ) -> Result<(Vec<Memory>, Vec<Edge>)> {
        use std::collections::HashSet;

        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(focus.to_string());
        let mut frontier: Vec<String> = vec![focus.to_string()];
        let mut edges: Vec<Edge> = Vec::new();
        let mut edge_ids: HashSet<String> = HashSet::new();

        let mut hop = 0;
        while hop < depth && !frontier.is_empty() && visited.len() < max_nodes {
            let mut next: Vec<String> = Vec::new();
            for chunk in frontier.chunks(SQL_IN_CHUNK) {
                let ph: Vec<String> = (1..=chunk.len()).map(|i| format!("?{i}")).collect();
                let ph = ph.join(", ");
                let sql = format!(
                    "SELECT id, src_id, dst_id, edge_type, label, weight, created_at
                     FROM edges WHERE src_id IN ({ph}) OR dst_id IN ({ph})"
                );
                let mut stmt = self.conn.prepare(&sql)?;
                let params_refs: Vec<&dyn rusqlite::types::ToSql> =
                    chunk.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();
                let found: Vec<Edge> = stmt
                    .query_map(params_refs.as_slice(), |row| row_to_edge(row))?
                    .collect::<rusqlite::Result<_>>()?;

                for edge in found {
                    if !edge_ids.insert(edge.id.clone()) {
                        continue;
                    }
                    for nid in [&edge.src_id, &edge.dst_id] {
                        if !visited.contains(nid.as_str()) && visited.len() < max_nodes {
                            visited.insert(nid.clone());
                            next.push(nid.clone());
                        }
                    }
                    edges.push(edge);
                }
            }
            frontier = next;
            hop += 1;
        }

        // Drop edges that reach past the node cap / depth boundary.
        edges.retain(|e| visited.contains(&e.src_id) && visited.contains(&e.dst_id));

        let ids: Vec<String> = visited.into_iter().collect();
        let nodes = self.memories_by_ids(&ids)?;
        Ok((nodes, edges))
    }

    /// Global graph sample for the initial explorer load: the `max_nodes`
    /// most recent memories plus all edges with both endpoints in the sample.
    pub fn graph_sample(&self, max_nodes: usize) -> Result<(Vec<Memory>, Vec<Edge>)> {
        use std::collections::HashSet;

        let mut stmt = self.conn.prepare(
            "SELECT id, content, memory_type, importance, project_id, source, metadata,
                    created_at, updated_at, accessed_at, access_count,
                    is_decoy, tier, strength, cold_path
             FROM memories ORDER BY created_at DESC LIMIT ?1",
        )?;
        let nodes: Vec<Memory> = stmt
            .query_map(params![max_nodes as i64], |row| row_to_memory(row))?
            .collect::<rusqlite::Result<_>>()?;

        let ids: HashSet<&str> = nodes.iter().map(|m| m.id.as_str()).collect();
        let id_vec: Vec<String> = nodes.iter().map(|m| m.id.clone()).collect();

        let mut edges: Vec<Edge> = Vec::new();
        for chunk in id_vec.chunks(SQL_IN_CHUNK) {
            let ph: Vec<String> = (1..=chunk.len()).map(|i| format!("?{i}")).collect();
            let sql = format!(
                "SELECT id, src_id, dst_id, edge_type, label, weight, created_at
                 FROM edges WHERE src_id IN ({})",
                ph.join(", ")
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let params_refs: Vec<&dyn rusqlite::types::ToSql> =
                chunk.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();
            let found = stmt
                .query_map(params_refs.as_slice(), |row| row_to_edge(row))?
                .collect::<rusqlite::Result<Vec<Edge>>>()?;
            edges.extend(found.into_iter().filter(|e| ids.contains(e.dst_id.as_str())));
        }

        Ok((nodes, edges))
    }

    /// Fetch memories by id (order unspecified). Chunks the IN list to stay
    /// under SQLite's parameter limit.
    fn memories_by_ids(&self, ids: &[String]) -> Result<Vec<Memory>> {
        let mut out = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(SQL_IN_CHUNK) {
            let ph: Vec<String> = (1..=chunk.len()).map(|i| format!("?{i}")).collect();
            let sql = format!(
                "SELECT id, content, memory_type, importance, project_id, source, metadata,
                        created_at, updated_at, accessed_at, access_count,
                        is_decoy, tier, strength, cold_path
                 FROM memories WHERE id IN ({})",
                ph.join(", ")
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let params_refs: Vec<&dyn rusqlite::types::ToSql> =
                chunk.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();
            let found: Vec<Memory> = stmt
                .query_map(params_refs.as_slice(), |row| row_to_memory(row))?
                .collect::<rusqlite::Result<_>>()?;
            out.extend(found);
        }
        Ok(out)
    }

    // -----------------------------------------------------------------------
    // Jobs
    // -----------------------------------------------------------------------

    pub fn create_job(&self, job_type: JobType, memory_id: &str) -> Result<Job> {
        let id = Uuid::now_v7().to_string();
        let now = chrono::Utc::now().to_rfc3339();

        self.conn.execute(
            "INSERT INTO jobs (id, job_type, memory_id, status, attempts, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'pending', 0, ?4, ?5)",
            params![id, job_type.to_string(), memory_id, now, now],
        )?;

        Ok(Job {
            id,
            job_type,
            memory_id: memory_id.to_string(),
            status: JobStatus::Pending,
            attempts: 0,
            last_error: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        })
    }

    pub fn get_pending_jobs(&self, limit: usize) -> Result<Vec<Job>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, job_type, memory_id, status, attempts, last_error, created_at, updated_at
             FROM jobs WHERE status = 'pending' ORDER BY created_at ASC LIMIT ?1",
        )?;
        let jobs = stmt.query_map(params![limit as i64], |row| row_to_job(row))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(jobs)
    }

    /// Claim a job: status → running, attempts += 1. The only place attempts
    /// are counted, so attempts == number of executions started.
    pub fn mark_job_running(&self, id: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE jobs SET status = 'running', attempts = attempts + 1, updated_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }

    /// Transition a job without touching `attempts`. Setting back to
    /// `pending` with an error is the retry path; `updated_at` doubles as
    /// the retry/backoff timestamp.
    pub fn update_job_status(&self, id: &str, status: JobStatus, error: Option<&str>) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE jobs SET status = ?1, last_error = ?2, updated_at = ?3 WHERE id = ?4",
            params![status.to_string(), error, now, id],
        )?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Stats
    // -----------------------------------------------------------------------

    pub fn stats(&self) -> Result<Stats> {
        let memory_count: i64 = self.conn.query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))?;
        let edge_count: i64 = self.conn.query_row("SELECT COUNT(*) FROM edges", [], |r| r.get(0))?;
        let project_count: i64 = self.conn.query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))?;
        let job_count: i64 = self.conn.query_row("SELECT COUNT(*) FROM jobs WHERE status = 'pending'", [], |r| r.get(0))?;

        let mut stmt = self.conn.prepare("SELECT memory_type, COUNT(*) FROM memories GROUP BY memory_type")?;
        let by_type: Vec<(String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;

        Ok(Stats { memory_count, edge_count, project_count, pending_jobs: job_count, by_type })
    }
}

// ---------------------------------------------------------------------------
// Code knowledge graph (Tree-sitter)
// ---------------------------------------------------------------------------

impl Store {
    pub fn cg_file_hash(&self, path: &str) -> Result<Option<String>> {
        match self.conn.query_row("SELECT content_hash FROM cg_files WHERE path = ?1", params![path], |r| r.get(0)) {
            Ok(hash) => Ok(Some(hash)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// All nodes across every file in one query — the no-focus dashboard
    /// path used to loop `cg_nodes_in_file` once per `cg_all_files` entry
    /// (N+1); `cg_nodes.file_path` already denormalizes the path so a plain
    /// `SELECT` is all that's needed.
    pub fn cg_all_nodes(&self, limit: Option<usize>) -> Result<Vec<CgNode>> {
        let sql = match limit {
            Some(n) => format!("SELECT id, file_path, kind, name, start_line, end_line FROM cg_nodes LIMIT {n}"),
            None => "SELECT id, file_path, kind, name, start_line, end_line FROM cg_nodes".to_string(),
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let nodes = stmt.query_map([], row_to_cg_node)?.collect::<rusqlite::Result<_>>()?;
        Ok(nodes)
    }

    pub fn cg_all_files(&self) -> Result<Vec<CgFile>> {
        let mut stmt = self.conn.prepare("SELECT path, language, content_hash FROM cg_files")?;
        let files = stmt
            .query_map([], |r| Ok(CgFile { path: r.get(0)?, language: r.get(1)?, content_hash: r.get(2)? }))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(files)
    }

    /// Remove a file's nodes/edges (cascade) and its `cg_files` row, e.g.
    /// before re-inserting fresh parse results or when the file is deleted.
    pub fn cg_clear_file(&self, path: &str) -> Result<()> {
        self.conn.execute("DELETE FROM cg_files WHERE path = ?1", params![path])?;
        Ok(())
    }

    pub fn cg_upsert_file(&self, file: &CgFile) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO cg_files (path, language, content_hash, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET language = ?2, content_hash = ?3, updated_at = ?4",
            params![file.path, file.language, file.content_hash, now],
        )?;
        Ok(())
    }

    pub fn cg_insert_node(&self, node: &CgNode) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO cg_nodes (id, file_path, kind, name, start_line, end_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![node.id, node.file_path, node.kind.to_string(), node.name, node.start_line, node.end_line],
        )?;
        Ok(())
    }

    /// Returns whether a new row was inserted (false if this exact edge
    /// already existed) — callers use this to report accurate edge counts.
    pub fn cg_insert_edge(&self, edge: &CgEdge) -> Result<bool> {
        let changed = self.conn.execute(
            "INSERT OR IGNORE INTO cg_edges (src_id, dst_id, kind) VALUES (?1, ?2, ?3)",
            params![edge.src_id, edge.dst_id, edge.kind.to_string()],
        )?;
        Ok(changed > 0)
    }

    pub fn cg_node(&self, id: &str) -> Result<Option<CgNode>> {
        let result = self.conn.query_row(
            "SELECT id, file_path, kind, name, start_line, end_line FROM cg_nodes WHERE id = ?1",
            params![id],
            row_to_cg_node,
        );
        match result {
            Ok(n) => Ok(Some(n)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// All nodes with an exact name match, across every file. Used to
    /// resolve call/test references found during parsing.
    pub fn cg_nodes_by_name(&self, name: &str, kinds: &[CgNodeKind]) -> Result<Vec<CgNode>> {
        let kind_list = kinds.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT id, file_path, kind, name, start_line, end_line FROM cg_nodes WHERE name = ?1 AND kind IN ({kind_list})"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let nodes = stmt.query_map(params![name], row_to_cg_node)?.collect::<rusqlite::Result<_>>()?;
        Ok(nodes)
    }

    pub fn cg_nodes_in_file(&self, path: &str) -> Result<Vec<CgNode>> {
        let mut stmt =
            self.conn.prepare("SELECT id, file_path, kind, name, start_line, end_line FROM cg_nodes WHERE file_path = ?1")?;
        let nodes = stmt.query_map(params![path], row_to_cg_node)?.collect::<rusqlite::Result<_>>()?;
        Ok(nodes)
    }

    /// Keyword search by substring, case-insensitive.
    pub fn cg_search_by_name(&self, keyword: &str, limit: usize) -> Result<Vec<CgNode>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, file_path, kind, name, start_line, end_line FROM cg_nodes
             WHERE name LIKE ?1 ESCAPE '\\' ORDER BY name LIMIT ?2",
        )?;
        let pattern = format!("%{}%", escape_like(keyword));
        let nodes = stmt.query_map(params![pattern, limit as i64], row_to_cg_node)?.collect::<rusqlite::Result<_>>()?;
        Ok(nodes)
    }

    /// Nodes reached by edges of `kind` pointing *into* `node_id` (e.g. who
    /// calls this, who imports this, who tests this).
    pub fn cg_edges_into(&self, node_id: &str, kind: CgEdgeKind) -> Result<Vec<CgNode>> {
        let mut stmt = self.conn.prepare(
            "SELECT n.id, n.file_path, n.kind, n.name, n.start_line, n.end_line
             FROM cg_edges e JOIN cg_nodes n ON n.id = e.src_id
             WHERE e.dst_id = ?1 AND e.kind = ?2",
        )?;
        let nodes = stmt.query_map(params![node_id, kind.to_string()], row_to_cg_node)?.collect::<rusqlite::Result<_>>()?;
        Ok(nodes)
    }

    /// Nodes reached by edges of `kind` pointing *out of* `node_id` (e.g.
    /// what this calls).
    pub fn cg_edges_out_of(&self, node_id: &str, kind: CgEdgeKind) -> Result<Vec<CgNode>> {
        let mut stmt = self.conn.prepare(
            "SELECT n.id, n.file_path, n.kind, n.name, n.start_line, n.end_line
             FROM cg_edges e JOIN cg_nodes n ON n.id = e.dst_id
             WHERE e.src_id = ?1 AND e.kind = ?2",
        )?;
        let nodes = stmt.query_map(params![node_id, kind.to_string()], row_to_cg_node)?.collect::<rusqlite::Result<_>>()?;
        Ok(nodes)
    }

    /// Edges with both endpoints in `ids` — used by the focus/blast-radius
    /// dashboard path so it never has to load every edge in the DB just to
    /// filter most of them out in memory.
    pub fn cg_edges_for_nodes(&self, ids: &[String]) -> Result<Vec<CgEdge>> {
        use std::collections::HashSet;
        let id_set: HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();
        let mut edges = Vec::new();
        for chunk in ids.chunks(SQL_IN_CHUNK) {
            let ph: Vec<String> = (1..=chunk.len()).map(|i| format!("?{i}")).collect();
            let sql = format!("SELECT src_id, dst_id, kind FROM cg_edges WHERE src_id IN ({})", ph.join(", "));
            let mut stmt = self.conn.prepare(&sql)?;
            let params_refs: Vec<&dyn rusqlite::types::ToSql> =
                chunk.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();
            let found = stmt
                .query_map(params_refs.as_slice(), |row| {
                    let kind: String = row.get(2)?;
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, kind))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (src_id, dst_id, kind) in found {
                if id_set.contains(dst_id.as_str()) {
                    edges.push(CgEdge { src_id, dst_id, kind: kind.parse().map_err(|e: anyhow::Error| anyhow::anyhow!(e))? });
                }
            }
        }
        Ok(edges)
    }

    pub fn cg_all_edges(&self) -> Result<Vec<CgEdge>> {
        let mut stmt = self.conn.prepare("SELECT src_id, dst_id, kind FROM cg_edges")?;
        let edges = stmt
            .query_map([], |r| {
                let kind: String = r.get(2)?;
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, kind))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        edges
            .into_iter()
            .map(|(src_id, dst_id, kind)| {
                Ok(CgEdge { src_id, dst_id, kind: kind.parse().map_err(|e: anyhow::Error| anyhow::anyhow!(e))? })
            })
            .collect()
    }

    pub fn cg_stats(&self) -> Result<(i64, i64, i64)> {
        let files: i64 = self.conn.query_row("SELECT COUNT(*) FROM cg_files", [], |r| r.get(0))?;
        let nodes: i64 = self.conn.query_row("SELECT COUNT(*) FROM cg_nodes", [], |r| r.get(0))?;
        let edges: i64 = self.conn.query_row("SELECT COUNT(*) FROM cg_edges", [], |r| r.get(0))?;
        Ok((files, nodes, edges))
    }
}

fn row_to_cg_node(row: &rusqlite::Row) -> rusqlite::Result<CgNode> {
    let kind: String = row.get(2)?;
    Ok(CgNode {
        id: row.get(0)?,
        file_path: row.get(1)?,
        kind: kind.parse().map_err(|e: anyhow::Error| {
            rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, e.into())
        })?,
        name: row.get(3)?,
        start_line: row.get(4)?,
        end_line: row.get(5)?,
    })
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

// ---------------------------------------------------------------------------
// SessionGroup
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct SessionGroup {
    pub session_id: Option<String>,
    pub project_id: Option<String>,
    pub started_at: String,
    pub ended_at: String,
    pub memories: Vec<Memory>,
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Stats {
    pub memory_count: i64,
    pub edge_count: i64,
    pub project_count: i64,
    pub pending_jobs: i64,
    /// (memory_type, count) pairs for the dashboard breakdown.
    pub by_type: Vec<(String, i64)>,
}

// ---------------------------------------------------------------------------
// Row mappers
// ---------------------------------------------------------------------------

fn row_to_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<Memory> {
    Ok(Memory {
        id: row.get(0)?,
        content: row.get(1)?,
        memory_type: MemoryType::from_str(&row.get::<_, String>(2)?).unwrap_or(MemoryType::Semantic),
        importance: row.get(3)?,
        project_id: row.get(4)?,
        source: Source::from_str(&row.get::<_, String>(5)?).unwrap_or(Source::Explicit),
        metadata: row.get::<_, Option<String>>(6)?.and_then(|s| serde_json::from_str(&s).ok()),
        created_at: parse_dt(&row.get::<_, String>(7)?),
        updated_at: parse_dt(&row.get::<_, String>(8)?),
        accessed_at: row.get::<_, Option<String>>(9)?.map(|s| parse_dt(&s)),
        access_count: row.get(10)?,
        is_decoy: row.get::<_, Option<i64>>(11)?.unwrap_or(0) != 0,
        tier: Tier::from_str(&row.get::<_, Option<String>>(12)?.unwrap_or_default()).unwrap_or(Tier::Hot),
        strength: row.get::<_, Option<f64>>(13)?.unwrap_or(1.0),
        cold_path: row.get(14)?,
    })
}

fn row_to_project(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        path: row.get(1)?,
        git_remote: row.get(2)?,
        name: row.get(3)?,
        created_at: parse_dt(&row.get::<_, String>(4)?),
        last_seen_at: parse_dt(&row.get::<_, String>(5)?),
    })
}

fn row_to_edge(row: &rusqlite::Row<'_>) -> rusqlite::Result<Edge> {
    Ok(Edge {
        id: row.get(0)?,
        src_id: row.get(1)?,
        dst_id: row.get(2)?,
        edge_type: EdgeType::from_str(&row.get::<_, String>(3)?).unwrap_or(EdgeType::Explicit),
        label: row.get(4)?,
        weight: row.get(5)?,
        created_at: parse_dt(&row.get::<_, String>(6)?),
    })
}

fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<Job> {
    Ok(Job {
        id: row.get(0)?,
        job_type: JobType::from_str(&row.get::<_, String>(1)?).unwrap_or(JobType::Summarize),
        memory_id: row.get(2)?,
        status: JobStatus::from_str(&row.get::<_, String>(3)?).unwrap_or(JobStatus::Pending),
        attempts: row.get(4)?,
        last_error: row.get(5)?,
        created_at: parse_dt(&row.get::<_, String>(6)?),
        updated_at: parse_dt(&row.get::<_, String>(7)?),
    })
}

fn parse_dt(s: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[test]
    fn create_and_get_memory() {
        let store = test_store();
        let mem = store.create_memory(
            "test content",
            MemoryType::Semantic,
            0.7,
            Source::Cli,
            None,
            None,
        ).unwrap();

        let fetched = store.get_memory(&mem.id).unwrap().unwrap();
        assert_eq!(fetched.content, "test content");
        assert_eq!(fetched.importance, 0.7);
        assert_eq!(fetched.memory_type, MemoryType::Semantic);
    }

    #[test]
    fn update_memory_content() {
        let store = test_store();
        let mem = store.create_memory("original", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        store.update_memory(&mem.id, "updated").unwrap();
        let fetched = store.get_memory(&mem.id).unwrap().unwrap();
        assert_eq!(fetched.content, "updated");
    }

    #[test]
    fn delete_memory_cascades() {
        let store = test_store();
        let mem = store.create_memory("delete me", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();

        // Index vec + fts
        let embedding = vec![0.1; 384];
        store.index_embedding(&mem.id, &embedding).unwrap();
        store.index_fts(&mem.id, "delete me").unwrap();

        // Verify indexed
        let vec_count: i64 = store.conn.query_row(
            "SELECT COUNT(*) FROM vec_memories WHERE memory_id = ?1", params![mem.id], |r| r.get(0)
        ).unwrap();
        assert_eq!(vec_count, 1);

        let fts_count: i64 = store.conn.query_row(
            "SELECT COUNT(*) FROM fts_memories WHERE memory_id = ?1", params![mem.id], |r| r.get(0)
        ).unwrap();
        assert_eq!(fts_count, 1);

        // Delete
        store.delete_memory(&mem.id).unwrap();

        // Verify cascaded
        let vec_count: i64 = store.conn.query_row(
            "SELECT COUNT(*) FROM vec_memories WHERE memory_id = ?1", params![mem.id], |r| r.get(0)
        ).unwrap();
        assert_eq!(vec_count, 0);

        let fts_count: i64 = store.conn.query_row(
            "SELECT COUNT(*) FROM fts_memories WHERE memory_id = ?1", params![mem.id], |r| r.get(0)
        ).unwrap();
        assert_eq!(fts_count, 0);

        assert!(store.get_memory(&mem.id).unwrap().is_none());
    }

    #[test]
    fn list_memories_filters() {
        let store = test_store();
        store.create_memory("a", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        store.create_memory("b", MemoryType::Preference, 0.5, Source::Cli, None, None).unwrap();
        store.create_memory("c", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();

        let (facts, total) = store.list_memories(None, Some("fact"), 10, 0).unwrap();
        assert_eq!(total, 2);
        assert_eq!(facts.len(), 2);

        let (all, total) = store.list_memories(None, None, 10, 0).unwrap();
        assert_eq!(total, 3);
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn project_upsert() {
        let store = test_store();
        let p = store.upsert_project("/home/user/project", "my-project", Some("https://github.com/user/repo")).unwrap();
        assert_eq!(p.name, "my-project");

        // Upsert same path with different name
        let p2 = store.upsert_project("/home/user/project", "renamed", None).unwrap();
        assert_eq!(p2.id, p.id);
        assert_eq!(p2.name, "renamed");
    }

    #[test]
    fn edge_unique_constraint() {
        let store = test_store();
        let m1 = store.create_memory("a", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        let m2 = store.create_memory("b", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();

        let e1 = store.create_edge(&m1.id, &m2.id, EdgeType::Similarity, None, 0.9).unwrap();
        assert!(e1.is_some());

        // Duplicate should return None (unique constraint)
        let e2 = store.create_edge(&m1.id, &m2.id, EdgeType::Similarity, None, 0.8).unwrap();
        assert!(e2.is_none());
    }

    #[test]
    fn migration_idempotent() {
        let store = test_store();
        // Running migrate again should not fail
        store.conn.execute("INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('schema_version', '1')", []).unwrap();
    }

    #[test]
    fn graph_neighborhood_bfs() {
        let store = test_store();
        // Chain: a — b — c — d
        let a = store.create_memory("a", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        let b = store.create_memory("b", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        let c = store.create_memory("c", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        let d = store.create_memory("d", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        store.create_edge(&a.id, &b.id, EdgeType::Similarity, None, 0.9).unwrap();
        store.create_edge(&b.id, &c.id, EdgeType::Similarity, None, 0.9).unwrap();
        store.create_edge(&c.id, &d.id, EdgeType::Similarity, None, 0.9).unwrap();

        // depth=1 from b: {a, b, c}, 2 edges
        let (nodes, edges) = store.graph_neighborhood(&b.id, 1, 500).unwrap();
        let ids: Vec<&str> = nodes.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(nodes.len(), 3);
        assert!(ids.contains(&a.id.as_str()) && ids.contains(&b.id.as_str()) && ids.contains(&c.id.as_str()));
        assert_eq!(edges.len(), 2);

        // depth=2 from b: all 4 nodes, 3 edges
        let (nodes, edges) = store.graph_neighborhood(&b.id, 2, 500).unwrap();
        assert_eq!(nodes.len(), 4);
        assert_eq!(edges.len(), 3);

        // max_nodes cap
        let (nodes, edges) = store.graph_neighborhood(&b.id, 2, 2).unwrap();
        assert_eq!(nodes.len(), 2);
        // Only edges with both endpoints inside the cap survive.
        for e in &edges {
            assert!(nodes.iter().any(|n| n.id == e.src_id));
            assert!(nodes.iter().any(|n| n.id == e.dst_id));
        }
    }

    #[test]
    fn graph_sample_excludes_dangling_edges() {
        let store = test_store();
        let a = store.create_memory("a", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        let b = store.create_memory("b", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        let c = store.create_memory("c", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        store.create_edge(&a.id, &b.id, EdgeType::Similarity, None, 0.9).unwrap();
        store.create_edge(&b.id, &c.id, EdgeType::Temporal, None, 1.0).unwrap();

        let (nodes, edges) = store.graph_sample(500).unwrap();
        assert_eq!(nodes.len(), 3);
        assert_eq!(edges.len(), 2);

        // Sample of 2 most-recent (b, c): only the b—c edge has both endpoints.
        let (nodes, edges) = store.graph_sample(2).unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].src_id, b.id);
        assert_eq!(edges[0].dst_id, c.id);
    }

    #[test]
    fn stats_work() {
        let store = test_store();
        store.create_memory("a", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        let s = store.stats().unwrap();
        assert_eq!(s.memory_count, 1);
    }

    #[test]
    fn list_sessions_groups_by_session_id_and_gap() {
        let store = test_store();

        // 3 memories with explicit session_id, 3 without (gap-split).
        store.create_memory(
            "a1", MemoryType::Fact, 0.5, Source::Cli, None,
            Some(&serde_json::json!({ "session_id": "s1" })),
        ).unwrap();
        store.create_memory(
            "a2", MemoryType::Fact, 0.5, Source::Cli, None,
            Some(&serde_json::json!({ "session_id": "s1" })),
        ).unwrap();
        store.create_memory(
            "a3", MemoryType::Fact, 0.5, Source::Cli, None,
            Some(&serde_json::json!({ "session_id": "s2" })),
        ).unwrap();

        // Unkeyed rows — within gap (default 1800s) → same session.
        store.create_memory("u1", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        store.create_memory("u2", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();

        let (sessions, total) = store.list_sessions(None, 1800, 100, 0).unwrap();
        assert_eq!(total, 3); // s1, s2, unkeyed group
        // s1 has 2 memories
        let s1 = sessions.iter().find(|s| s.session_id.as_deref() == Some("s1")).unwrap();
        assert_eq!(s1.memories.len(), 2);
        // s2 has 1 memory
        let s2 = sessions.iter().find(|s| s.session_id.as_deref() == Some("s2")).unwrap();
        assert_eq!(s2.memories.len(), 1);
        // Unkeyed group has 2
        let unkeyed = sessions.iter().find(|s| s.session_id.is_none()).unwrap();
        assert_eq!(unkeyed.memories.len(), 2);
    }

    #[test]
    fn list_sessions_legacy_extra_session_id() {
        let store = test_store();
        store.create_memory(
            "legacy", MemoryType::Fact, 0.5, Source::Cli, None,
            Some(&serde_json::json!({ "extra": { "session_id": "old-sid" } })),
        ).unwrap();

        let (sessions, total) = store.list_sessions(None, 1800, 100, 0).unwrap();
        assert_eq!(total, 1);
        assert_eq!(sessions[0].session_id.as_deref(), Some("old-sid"));
    }

    #[test]
    fn list_sessions_gap_split() {
        let store = test_store();
        // Create two unkeyed memories 2 hours apart.
        let m1 = store.create_memory("old", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        let m2 = store.create_memory("new", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();

        let two_hours_ago = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        store.conn.execute(
            "UPDATE memories SET created_at = ?1 WHERE id = ?2",
            rusqlite::params![two_hours_ago, m1.id],
        ).unwrap();

        // With 30-min gap → two sessions.
        let (sessions, total) = store.list_sessions(None, 1800, 100, 0).unwrap();
        assert_eq!(total, 2);
        assert!(sessions.iter().all(|s| s.session_id.is_none()));
    }

    #[test]
    fn list_sessions_project_filter() {
        let store = test_store();
        let p = store.upsert_project("/p", "proj", None).unwrap();
        store.create_memory(
            "a", MemoryType::Fact, 0.5, Source::Cli, Some(&p.id),
            Some(&serde_json::json!({ "session_id": "s1" })),
        ).unwrap();
        store.create_memory("b", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();

        // Filter by project → only s1.
        let (sessions, total) = store.list_sessions(Some(&p.id), 1800, 100, 0).unwrap();
        assert_eq!(total, 1);
        assert_eq!(sessions[0].session_id.as_deref(), Some("s1"));
    }

    #[test]
    fn list_sessions_ordering() {
        let store = test_store();
        // s2 created before s1 by backdating.
        store.create_memory(
            "new", MemoryType::Fact, 0.5, Source::Cli, None,
            Some(&serde_json::json!({ "session_id": "s1" })),
        ).unwrap();
        store.create_memory(
            "old", MemoryType::Fact, 0.5, Source::Cli, None,
            Some(&serde_json::json!({ "session_id": "s2" })),
        ).unwrap();

        // Backdate s2's memory.
        let two_days_ago = (chrono::Utc::now() - chrono::Duration::days(2)).to_rfc3339();
        let all = store.list_memories(None, None, 10, 0).unwrap().0;
        let old_mem = all.iter().find(|m| m.content == "old").unwrap();
        store.conn.execute(
            "UPDATE memories SET created_at = ?1 WHERE id = ?2",
            rusqlite::params![two_days_ago, old_mem.id],
        ).unwrap();

        let (sessions, _) = store.list_sessions(None, 1800, 100, 0).unwrap();
        // Newest first → s1 before s2.
        assert_eq!(sessions[0].session_id.as_deref(), Some("s1"));
        assert_eq!(sessions[1].session_id.as_deref(), Some("s2"));
    }

    // -----------------------------------------------------------------------
    // Failure injection tests
    // -----------------------------------------------------------------------

    #[test]
    fn double_delete_returns_false() {
        let store = test_store();
        let m = store.create_memory("x", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        assert!(store.delete_memory(&m.id).unwrap());
        assert!(!store.delete_memory(&m.id).unwrap());
    }

    #[test]
    fn get_nonexistent_memory_returns_none() {
        let store = test_store();
        assert!(store.get_memory("no-such-id").unwrap().is_none());
    }

    #[test]
    fn update_nonexistent_memory_returns_false() {
        let store = test_store();
        assert!(!store.update_memory("no-such-id", "new").unwrap());
    }

    #[test]
    fn merge_metadata_on_missing_memory_returns_false() {
        let store = test_store();
        let result = store.merge_metadata("no-such-id", &serde_json::json!({"x": 1})).unwrap();
        assert!(!result);
    }

    #[test]
    fn list_memories_empty_db() {
        let store = test_store();
        let (memories, total) = store.list_memories(None, None, 100, 0).unwrap();
        assert_eq!(total, 0);
        assert!(memories.is_empty());
    }

    #[test]
    fn stats_fresh_db() {
        let store = test_store();
        let s = store.stats().unwrap();
        assert_eq!(s.memory_count, 0);
        assert_eq!(s.edge_count, 0);
        assert_eq!(s.project_count, 0);
        assert_eq!(s.pending_jobs, 0);
    }

    #[test]
    fn corrupt_metadata_json_handled() {
        let store = test_store();
        let id = Uuid::now_v7().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        // Insert a memory with malformed metadata JSON.
        store.conn.execute(
            "INSERT INTO memories (id, content, memory_type, importance, project_id, source, metadata, created_at, updated_at, access_count)
             VALUES (?1, 'test', 'fact', 0.5, NULL, 'cli', ?2, ?3, ?3, 0)",
            rusqlite::params![id, "NOT VALID JSON {{{", now],
        ).unwrap();

        // Should still be fetchable; metadata parses to None.
        let mem = store.get_memory(&id).unwrap().unwrap();
        assert!(mem.metadata.is_none());
    }

    #[test]
    fn set_importance_clamped() {
        let store = test_store();
        let m = store.create_memory("x", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        store.set_importance(&m.id, 5.0).unwrap();
        let mem = store.get_memory(&m.id).unwrap().unwrap();
        assert_eq!(mem.importance, 1.0);

        store.set_importance(&m.id, -1.0).unwrap();
        let mem = store.get_memory(&m.id).unwrap().unwrap();
        assert_eq!(mem.importance, 0.0);
    }

    #[test]
    fn compressed_content_round_trips() {
        let store = test_store();
        let m = store.create_memory("hello world", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();

        assert!(store.get_compressed_content(&m.id).unwrap().is_none());

        store.set_compressed_content(&m.id, "h3ll0 w0rld", "caveman").unwrap();
        let (text, mode) = store.get_compressed_content(&m.id).unwrap().unwrap();
        assert_eq!(text, "h3ll0 w0rld");
        assert_eq!(mode, "caveman");

        // content itself must be untouched.
        let mem = store.get_memory(&m.id).unwrap().unwrap();
        assert_eq!(mem.content, "hello world");

        store.clear_compressed_content(&m.id).unwrap();
        assert!(store.get_compressed_content(&m.id).unwrap().is_none());
    }

    #[test]
    fn schema_version_is_4_with_compression_columns() {
        let store = test_store();
        assert_eq!(SCHEMA_VERSION, 4);
        let m = store.create_memory("x", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        // Columns exist and are queryable (NULL until set).
        let (compressed, mode): (Option<String>, Option<String>) = store
            .conn
            .query_row(
                "SELECT compressed_content, compression_mode FROM memories WHERE id = ?1",
                params![m.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(compressed.is_none());
        assert!(mode.is_none());
    }

    #[test]
    fn delete_memory_cascades_edges() {
        let store = test_store();
        let a = store.create_memory("a", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        let b = store.create_memory("b", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        store.create_edge(&a.id, &b.id, EdgeType::Similarity, None, 0.9).unwrap();

        let edge_count_before: i64 = store.conn.query_row("SELECT COUNT(*) FROM edges", [], |r| r.get(0)).unwrap();
        assert_eq!(edge_count_before, 1);

        store.delete_memory(&a.id).unwrap();

        let edge_count_after: i64 = store.conn.query_row("SELECT COUNT(*) FROM edges", [], |r| r.get(0)).unwrap();
        assert_eq!(edge_count_after, 0);
    }

    #[test]
    fn graph_sample_empty_db() {
        let store = test_store();
        let (nodes, edges) = store.graph_sample(100).unwrap();
        assert!(nodes.is_empty());
        assert!(edges.is_empty());
    }

    #[test]
    fn graph_neighborhood_unknown_focus() {
        let store = test_store();
        let result = store.graph_neighborhood("no-such-id", 1, 100);
        // Should return empty, not error.
        let (nodes, edges) = result.unwrap();
        assert!(nodes.is_empty());
        assert!(edges.is_empty());
    }

    #[test]
    fn cg_all_nodes_returns_nodes_across_files() {
        let store = test_store();
        store.cg_upsert_file(&CgFile { path: "a.rs".into(), language: "rust".into(), content_hash: "h1".into() }).unwrap();
        store.cg_upsert_file(&CgFile { path: "b.rs".into(), language: "rust".into(), content_hash: "h2".into() }).unwrap();
        store
            .cg_insert_node(&CgNode { id: "a#1:a".into(), file_path: "a.rs".into(), kind: CgNodeKind::Function, name: "a".into(), start_line: 1, end_line: 1 })
            .unwrap();
        store
            .cg_insert_node(&CgNode { id: "b#1:b".into(), file_path: "b.rs".into(), kind: CgNodeKind::Function, name: "b".into(), start_line: 1, end_line: 1 })
            .unwrap();

        let nodes = store.cg_all_nodes(None).unwrap();
        assert_eq!(nodes.len(), 2);

        let capped = store.cg_all_nodes(Some(1)).unwrap();
        assert_eq!(capped.len(), 1);
    }

    #[test]
    fn cg_edges_for_nodes_excludes_edges_with_endpoint_outside_set() {
        let store = test_store();
        store.cg_upsert_file(&CgFile { path: "a.rs".into(), language: "rust".into(), content_hash: "h1".into() }).unwrap();
        for (id, name) in [("a#1:a", "a"), ("a#2:b", "b"), ("a#3:c", "c")] {
            store
                .cg_insert_node(&CgNode { id: id.into(), file_path: "a.rs".into(), kind: CgNodeKind::Function, name: name.into(), start_line: 1, end_line: 1 })
                .unwrap();
        }
        store.cg_insert_edge(&CgEdge { src_id: "a#1:a".into(), dst_id: "a#2:b".into(), kind: CgEdgeKind::Calls }).unwrap();
        store.cg_insert_edge(&CgEdge { src_id: "a#1:a".into(), dst_id: "a#3:c".into(), kind: CgEdgeKind::Calls }).unwrap();

        // Only a and b are "in view" — the edge to c must be excluded.
        let edges = store.cg_edges_for_nodes(&["a#1:a".to_string(), "a#2:b".to_string()]).unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].dst_id, "a#2:b");
    }
}

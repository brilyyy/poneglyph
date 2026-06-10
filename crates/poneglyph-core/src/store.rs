use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use std::path::Path;
use std::str::FromStr;
use tracing::info;
use uuid::Uuid;
use crate::model::*;

pub const SCHEMA_VERSION: i64 = 1;

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
            "INSERT INTO memories (id, content, memory_type, importance, project_id, source, metadata, created_at, updated_at, access_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0)",
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
        })
    }

    pub fn get_memory(&self, id: &str) -> Result<Option<Memory>> {
        let result = self.conn.query_row(
            "SELECT id, content, memory_type, importance, project_id, source, metadata, created_at, updated_at, accessed_at, access_count
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
            "SELECT id, content, memory_type, importance, project_id, source, metadata, created_at, updated_at, accessed_at, access_count
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
            "SELECT id, content, memory_type, importance, project_id, source, metadata, created_at, updated_at, accessed_at, access_count
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
                "SELECT id, content, memory_type, importance, project_id, source, metadata, created_at, updated_at, accessed_at, access_count
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

    pub fn update_job_status(&self, id: &str, status: JobStatus, error: Option<&str>) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE jobs SET status = ?1, last_error = ?2, updated_at = ?3, attempts = attempts + 1 WHERE id = ?4",
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
        Ok(Stats { memory_count, edge_count, project_count, pending_jobs: job_count })
    }
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
}

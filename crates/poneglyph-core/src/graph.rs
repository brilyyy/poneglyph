//! Knowledge graph — no-LLM edge builders (PRD §8.4).
//!
//! All builders are deterministic, run with zero tokens, and are safe to
//! re-run: `Store::create_edge` uses INSERT OR IGNORE against the unique
//! (src, dst, type, label) constraint, and symmetric edge types store the
//! pair in canonical (min, max) order so recomputation never duplicates.

use anyhow::Result;
use rusqlite::params;

use crate::config::MemoryEdgesConfig;
use crate::model::EdgeType;
use crate::store::Store;

/// How many nearest neighbours to consider for similarity edges.
const SIMILARITY_CANDIDATES: usize = 20;

/// Tag-overlap edges need either ≥2 shared tags or a high Jaccard ratio —
/// one shared generic tag (e.g. "architecture") among several otherwise
/// unrelated tags bridges unrelated projects and is noise, not signal.
const MIN_SHARED_TAGS: usize = 2;
const MIN_TAG_JACCARD: f64 = 0.5;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += (*x as f64) * (*y as f64);
        na += (*x as f64) * (*x as f64);
        nb += (*y as f64) * (*y as f64);
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

fn bytes_to_vec(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn get_embedding(store: &Store, memory_id: &str) -> Result<Option<Vec<f32>>> {
    let result = store.conn.query_row(
        "SELECT embedding FROM vec_memories WHERE memory_id = ?1",
        params![memory_id],
        |row| row.get::<_, Vec<u8>>(0),
    );
    match result {
        Ok(bytes) => Ok(Some(bytes_to_vec(&bytes))),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Symmetric edge types store (min, max) so A→B and B→A collapse into one row.
fn canonical<'a>(a: &'a str, b: &'a str) -> (&'a str, &'a str) {
    if a <= b { (a, b) } else { (b, a) }
}

fn tags_of(metadata: Option<&serde_json::Value>) -> Vec<String> {
    metadata
        .and_then(|m| m.get("tags"))
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------

/// Caller-provided explicit links from `src_id` to each id in `linked_ids`.
/// Returns the number of edges actually created.
pub fn build_explicit_edges(store: &Store, src_id: &str, linked_ids: &[String]) -> Result<usize> {
    let mut created = 0;
    for dst in linked_ids {
        if dst == src_id {
            continue;
        }
        if store
            .create_edge(src_id, dst, EdgeType::Explicit, None, 1.0)?
            .is_some()
        {
            created += 1;
        }
    }
    Ok(created)
}

/// Cosine similarity ≥ threshold against the memory's nearest neighbours.
/// No-op when the memory has no embedding (e.g. stored without a model).
pub fn build_similarity_edges(store: &Store, memory_id: &str, threshold: f64) -> Result<usize> {
    let Some(target) = get_embedding(store, memory_id)? else {
        return Ok(0);
    };

    let target_bytes: Vec<u8> = target.iter().flat_map(|f| f.to_le_bytes()).collect();
    let mut stmt = store.conn.prepare(
        "SELECT memory_id FROM vec_memories WHERE embedding MATCH ?1 AND k = ?2",
    )?;
    let candidates: Vec<String> = stmt
        .query_map(params![target_bytes, (SIMILARITY_CANDIDATES + 1) as i64], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;

    let mut created = 0;
    for cand in candidates {
        if cand == memory_id {
            continue;
        }
        let Some(other) = get_embedding(store, &cand)? else {
            continue;
        };
        let cos = cosine(&target, &other);
        if cos >= threshold {
            let (a, b) = canonical(memory_id, &cand);
            if store
                .create_edge(a, b, EdgeType::Similarity, None, cos)?
                .is_some()
            {
                created += 1;
            }
        }
    }
    Ok(created)
}

/// Memories created within `window_secs` of this one, in the same project.
/// Memories without a project are skipped (a global time window is noise).
pub fn build_temporal_edges(store: &Store, memory_id: &str, window_secs: i64) -> Result<usize> {
    let Some(mem) = store.get_memory(memory_id)? else {
        return Ok(0);
    };
    let Some(project_id) = &mem.project_id else {
        return Ok(0);
    };

    let window = chrono::Duration::seconds(window_secs);
    // RFC3339 UTC strings compare lexicographically.
    let low = (mem.created_at - window).to_rfc3339();
    let high = (mem.created_at + window).to_rfc3339();

    let mut stmt = store.conn.prepare(
        "SELECT id FROM memories
         WHERE project_id = ?1 AND id != ?2 AND created_at BETWEEN ?3 AND ?4",
    )?;
    let neighbors: Vec<String> = stmt
        .query_map(params![project_id, memory_id, low, high], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;

    let mut created = 0;
    for other in neighbors {
        let (a, b) = canonical(memory_id, &other);
        if store
            .create_edge(a, b, EdgeType::Temporal, None, 1.0)?
            .is_some()
        {
            created += 1;
        }
    }
    Ok(created)
}

/// Edges to memories sharing at least one tag; weight = Jaccard overlap.
pub fn build_tag_overlap_edges(store: &Store, memory_id: &str) -> Result<usize> {
    let Some(mem) = store.get_memory(memory_id)? else {
        return Ok(0);
    };
    let tags = tags_of(mem.metadata.as_ref());
    if tags.is_empty() {
        return Ok(0);
    }

    let placeholders: Vec<String> = (0..tags.len()).map(|i| format!("?{}", i + 2)).collect();
    let sql = format!(
        "SELECT DISTINCT m.id, m.metadata
         FROM memories m, json_each(m.metadata, '$.tags') je
         WHERE m.id != ?1 AND je.value IN ({})",
        placeholders.join(", ")
    );

    let mut stmt = store.conn.prepare(&sql)?;
    let mut sql_params: Vec<Box<dyn rusqlite::types::ToSql>> =
        vec![Box::new(memory_id.to_string())];
    for t in &tags {
        sql_params.push(Box::new(t.clone()));
    }
    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        sql_params.iter().map(|p| p.as_ref()).collect();

    let candidates: Vec<(String, Option<String>)> = stmt
        .query_map(param_refs.as_slice(), |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;

    let my_tags: std::collections::HashSet<&str> = tags.iter().map(|s| s.as_str()).collect();
    let mut created = 0;
    for (other_id, other_meta) in candidates {
        let other_meta: Option<serde_json::Value> =
            other_meta.and_then(|s| serde_json::from_str(&s).ok());
        let other_tags = tags_of(other_meta.as_ref());
        if other_tags.is_empty() {
            continue;
        }
        let other_set: std::collections::HashSet<&str> =
            other_tags.iter().map(|s| s.as_str()).collect();
        let shared = my_tags.intersection(&other_set).count();
        if shared == 0 {
            continue;
        }
        let jaccard = shared as f64 / my_tags.union(&other_set).count() as f64;
        if shared < MIN_SHARED_TAGS && jaccard < MIN_TAG_JACCARD {
            continue;
        }
        let (a, b) = canonical(memory_id, &other_id);
        if store
            .create_edge(a, b, EdgeType::TagOverlap, None, jaccard)?
            .is_some()
        {
            created += 1;
        }
    }
    Ok(created)
}

/// Top-k nearest neighbours of a memory by embedding (excluding itself).
/// Empty when the memory has no embedding (FTS-only mode).
pub fn nearest_neighbors(store: &Store, memory_id: &str, k: usize) -> Result<Vec<crate::model::Memory>> {
    let Some(target) = get_embedding(store, memory_id)? else {
        return Ok(Vec::new());
    };

    let target_bytes: Vec<u8> = target.iter().flat_map(|f| f.to_le_bytes()).collect();
    let mut stmt = store.conn.prepare(
        "SELECT memory_id FROM vec_memories WHERE embedding MATCH ?1 AND k = ?2",
    )?;
    let candidates: Vec<String> = stmt
        .query_map(params![target_bytes, (k + 1) as i64], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;

    let mut out = Vec::with_capacity(k);
    for cand in candidates {
        if cand == memory_id {
            continue;
        }
        if let Some(m) = store.get_memory(&cand)? {
            out.push(m);
        }
        if out.len() == k {
            break;
        }
    }
    Ok(out)
}

/// Run every no-LLM builder for one memory. Used by the enrichment worker.
pub fn build_edges_for_memory(store: &Store, cfg: &MemoryEdgesConfig, memory_id: &str) -> Result<usize> {
    let mut n = 0;
    n += build_similarity_edges(store, memory_id, cfg.similarity_threshold)?;
    n += build_temporal_edges(store, memory_id, cfg.temporal_window_secs)?;
    n += build_tag_overlap_edges(store, memory_id)?;
    Ok(n)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MemoryType, Source};

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    fn mem(store: &Store, content: &str, project: Option<&str>, tags: &[&str]) -> String {
        let metadata = if tags.is_empty() {
            None
        } else {
            Some(serde_json::json!({ "tags": tags }))
        };
        store
            .create_memory(content, MemoryType::Fact, 0.5, Source::Cli, project, metadata.as_ref())
            .unwrap()
            .id
    }

    /// 384-dim unit-ish vector pointing mostly along `axis` with `noise` on axis+1.
    fn vec_along(axis: usize, noise: f32) -> Vec<f32> {
        let mut v = vec![0.0f32; 384];
        v[axis] = 1.0;
        v[(axis + 1) % 384] = noise;
        v
    }

    #[test]
    fn cosine_basics() {
        let a = vec_along(0, 0.0);
        let b = vec_along(0, 0.0);
        let c = vec_along(1, 0.0);
        assert!((cosine(&a, &b) - 1.0).abs() < 1e-9);
        assert!(cosine(&a, &c).abs() < 1e-9);
    }

    #[test]
    fn similarity_edge_for_near_duplicates() {
        let s = store();
        let m1 = mem(&s, "near duplicate one", None, &[]);
        let m2 = mem(&s, "near duplicate two", None, &[]);
        let m3 = mem(&s, "unrelated", None, &[]);

        s.index_embedding(&m1, &vec_along(0, 0.05)).unwrap();
        s.index_embedding(&m2, &vec_along(0, 0.10)).unwrap(); // cos(m1,m2) ≈ 0.99
        s.index_embedding(&m3, &vec_along(7, 0.0)).unwrap(); // orthogonal

        let created = build_similarity_edges(&s, &m1, 0.82).unwrap();
        assert_eq!(created, 1, "only the near-duplicate qualifies");

        let edges = s.get_edges_for_memory(&m1).unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].edge_type, EdgeType::Similarity);
        assert!(edges[0].weight > 0.95);

        // AC2: recompute (from either side) creates no duplicates.
        assert_eq!(build_similarity_edges(&s, &m1, 0.82).unwrap(), 0);
        assert_eq!(build_similarity_edges(&s, &m2, 0.82).unwrap(), 0);
        assert_eq!(s.get_edges_for_memory(&m1).unwrap().len(), 1);
    }

    #[test]
    fn similarity_noop_without_embedding() {
        let s = store();
        let m1 = mem(&s, "no embedding", None, &[]);
        assert_eq!(build_similarity_edges(&s, &m1, 0.82).unwrap(), 0);
    }

    #[test]
    fn temporal_edges_same_project_within_window() {
        let s = store();
        let p = s.upsert_project("/p", "p", None).unwrap();
        let q = s.upsert_project("/q", "q", None).unwrap();

        let m1 = mem(&s, "first", Some(&p.id), &[]);
        let m2 = mem(&s, "second", Some(&p.id), &[]); // same project, same instant
        let m3 = mem(&s, "other project", Some(&q.id), &[]);
        let m4 = mem(&s, "no project", None, &[]);

        let created = build_temporal_edges(&s, &m1, 300).unwrap();
        assert_eq!(created, 1, "only same-project neighbour within window");

        let edges = s.get_edges_for_memory(&m1).unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].edge_type, EdgeType::Temporal);
        // Canonical ordering: src < dst, endpoints are exactly {m1, m2}.
        assert!(edges[0].src_id < edges[0].dst_id);
        let endpoints = [edges[0].src_id.as_str(), edges[0].dst_id.as_str()];
        assert!(endpoints.contains(&m1.as_str()) && endpoints.contains(&m2.as_str()));

        assert_eq!(build_temporal_edges(&s, &m3, 300).unwrap(), 0);
        assert_eq!(build_temporal_edges(&s, &m4, 300).unwrap(), 0);

        // Recompute: no duplicates.
        assert_eq!(build_temporal_edges(&s, &m1, 300).unwrap(), 0);
    }

    #[test]
    fn temporal_respects_window() {
        let s = store();
        let p = s.upsert_project("/p", "p", None).unwrap();
        let m1 = mem(&s, "old", Some(&p.id), &[]);
        let m2 = mem(&s, "new", Some(&p.id), &[]);

        // Push m1 outside the 5-minute window.
        let old = (chrono::Utc::now() - chrono::Duration::seconds(600)).to_rfc3339();
        s.conn
            .execute("UPDATE memories SET created_at = ?1 WHERE id = ?2", params![old, m1])
            .unwrap();

        assert_eq!(build_temporal_edges(&s, &m2, 300).unwrap(), 0);
    }

    #[test]
    fn tag_overlap_edges_with_jaccard_weight() {
        let s = store();
        let m1 = mem(&s, "rust memory engine", None, &["rust", "memory"]);
        let m2 = mem(&s, "rust cli tooling", None, &["rust", "memory", "cli"]);
        let m3 = mem(&s, "cooking pasta", None, &["food"]);
        let m4 = mem(&s, "untagged", None, &[]);

        let created = build_tag_overlap_edges(&s, &m1).unwrap();
        assert_eq!(created, 1, "2 shared tags clears the noise floor");

        let edges = s.get_edges_for_memory(&m1).unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].edge_type, EdgeType::TagOverlap);
        // {rust, memory} / {rust, memory, cli} = 2/3
        assert!((edges[0].weight - 2.0 / 3.0).abs() < 1e-9);

        assert_eq!(build_tag_overlap_edges(&s, &m3).unwrap(), 0);
        assert_eq!(build_tag_overlap_edges(&s, &m4).unwrap(), 0);

        // Recompute from the other side: same canonical row, no duplicate.
        assert_eq!(build_tag_overlap_edges(&s, &m2).unwrap(), 0);
    }

    #[test]
    fn tag_overlap_skips_single_generic_tag_noise() {
        let s = store();
        // One shared tag ("architecture") buried among unrelated tags should
        // not bridge two otherwise unrelated memories (low Jaccard, <2 shared).
        let m1 = mem(&s, "switched to grpc", None, &["grpc", "architecture"]);
        let m2 = mem(&s, "use app router", None, &["nextjs", "architecture"]);
        assert_eq!(build_tag_overlap_edges(&s, &m1).unwrap(), 0);
        assert_eq!(s.get_edges_for_memory(&m1).unwrap().len(), 0);

        // But a single shared tag where it's the *only* tag on both sides
        // (jaccard 1.0) is a real signal, not noise.
        let m3 = mem(&s, "first", None, &["onboarding"]);
        let m4 = mem(&s, "second", None, &["onboarding"]);
        assert_eq!(build_tag_overlap_edges(&s, &m3).unwrap(), 1);
        let _ = m2;
        let _ = m4;
    }

    #[test]
    fn explicit_edges_skip_self_and_duplicates() {
        let s = store();
        let m1 = mem(&s, "a", None, &[]);
        let m2 = mem(&s, "b", None, &[]);

        let created =
            build_explicit_edges(&s, &m1, &[m2.clone(), m1.clone(), m2.clone()]).unwrap();
        assert_eq!(created, 1);
        assert_eq!(s.get_edges_for_memory(&m1).unwrap().len(), 1);
    }

    #[test]
    fn build_all_combines_builders() {
        let s = store();
        let cfg = MemoryEdgesConfig::default();
        let p = s.upsert_project("/p", "p", None).unwrap();

        let m1 = mem(&s, "alpha", Some(&p.id), &["x"]);
        let m2 = mem(&s, "beta", Some(&p.id), &["x"]);
        s.index_embedding(&m1, &vec_along(0, 0.05)).unwrap();
        s.index_embedding(&m2, &vec_along(0, 0.06)).unwrap();

        let n = build_edges_for_memory(&s, &cfg, &m2).unwrap();
        // similarity + temporal + tag_overlap, each one edge between m1/m2
        assert_eq!(n, 3);

        // Idempotent on recompute.
        assert_eq!(build_edges_for_memory(&s, &cfg, &m2).unwrap(), 0);
        assert_eq!(build_edges_for_memory(&s, &cfg, &m1).unwrap(), 0);
    }
}

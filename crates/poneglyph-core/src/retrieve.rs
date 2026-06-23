use std::collections::HashMap;
use std::str::FromStr;

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::config::RetrievalConfig;
use crate::model::{Memory, MemoryType, Source};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallResult {
    pub memory: Memory,
    pub score: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecallFilters {
    pub memory_type: Option<String>,
    pub project_id: Option<String>,
    pub since: Option<String>,
    pub tag: Option<String>,
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

const RRF_K: f64 = 60.0;
const GRAPH_HOP_DECAY: f64 = 0.5;

fn recency_factor(created_at: DateTime<Utc>) -> f64 {
    let now = Utc::now();
    let days = (now - created_at).num_seconds() as f64 / 86400.0;
    (1.0 - days / 30.0).max(0.0)
}

fn strength_factor(strength: f64) -> f64 {
    // Ebbinghaus-inspired: memories with low strength get penalized
    // strength=1.0 → 1.0, strength=0.3 → 0.5, strength=0.0 → 0.1
    0.1 + 0.9 * strength
}

fn final_score(rrf_score: f64, importance: f64, created_at: DateTime<Utc>, strength: f64) -> f64 {
    rrf_score * (1.0 + 0.1 * importance + 0.05 * recency_factor(created_at)) * strength_factor(strength)
}

fn row_to_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<Memory> {
    let id: String = row.get(0)?;
    let content: String = row.get(1)?;
    let memory_type: String = row.get(2)?;
    let importance: f64 = row.get(3)?;
    let project_id: Option<String> = row.get(4)?;
    let source: String = row.get(5)?;
    let metadata: Option<String> = row.get(6)?;
    let created_at: String = row.get(7)?;
    let updated_at: String = row.get(8)?;
    let accessed_at: Option<String> = row.get(9)?;
    let access_count: i64 = row.get(10)?;

    Ok(Memory {
        id,
        content,
        memory_type: MemoryType::from_str(&memory_type).unwrap_or(MemoryType::Semantic),
        importance,
        project_id,
        source: Source::from_str(&source).unwrap_or(Source::Explicit),
        metadata: metadata.and_then(|s| serde_json::from_str(&s).ok()),
        created_at: DateTime::parse_from_rfc3339(&created_at)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        updated_at: DateTime::parse_from_rfc3339(&updated_at)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        accessed_at: accessed_at.and_then(|s| {
            DateTime::parse_from_rfc3339(&s)
                .map(|dt| dt.with_timezone(&Utc))
                .ok()
        }),
        access_count,
        is_decoy: row.get::<_, Option<i64>>(11)?.unwrap_or(0) != 0,
        tier: crate::model::Tier::from_str(&row.get::<_, Option<String>>(12)?.unwrap_or_default()).unwrap_or(crate::model::Tier::Hot),
        strength: row.get::<_, Option<f64>>(13)?.unwrap_or(1.0),
        cold_path: row.get(14)?,
    })
}

const MEMORY_COLS: &str = "id, content, memory_type, importance, project_id, source, metadata, created_at, updated_at, accessed_at, access_count, is_decoy, tier, strength, cold_path";

fn build_filter_where(filters: &RecallFilters) -> (String, Vec<String>) {
    let mut conditions = Vec::new();
    let mut values = Vec::new();

    if let Some(ref mt) = filters.memory_type {
        conditions.push("memory_type = ?".to_string());
        values.push(mt.clone());
    }
    if let Some(ref pid) = filters.project_id {
        conditions.push("project_id = ?".to_string());
        values.push(pid.clone());
    }
    if let Some(ref since) = filters.since {
        conditions.push("created_at >= ?".to_string());
        values.push(since.clone());
    }
    if let Some(ref tag) = filters.tag {
        conditions.push("json_each.value = ?".to_string());
        values.push(tag.clone());
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    (where_clause, values)
}

// ---------------------------------------------------------------------------
// Retrieval paths
// ---------------------------------------------------------------------------

fn dense_recall(
    conn: &rusqlite::Connection,
    query_vec: &[f32],
    limit: usize,
) -> Result<Vec<(String, f64)>> {
    let query_bytes: Vec<u8> = query_vec.iter().flat_map(|f| f.to_le_bytes()).collect();

    let mut stmt = conn.prepare(
        "SELECT memory_id, distance FROM vec_memories WHERE embedding MATCH ?1 AND k = ?2 ORDER BY distance",
    )?;

    let results: Vec<(String, f64)> = stmt
        .query_map(params![query_bytes, limit as i64], |row| {
            let id: String = row.get(0)?;
            let distance: f64 = row.get(1)?;
            Ok((id, distance))
        })?
        .collect::<rusqlite::Result<_>>()?;

    Ok(results)
}

/// FTS5 MATCH has its own query syntax; raw natural-language input (apostrophes,
/// `?`, `-`) is a syntax error. Quote each alphanumeric token and OR them.
fn sanitize_fts_query(query: &str) -> String {
    query
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\""))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn sparse_recall(
    conn: &rusqlite::Connection,
    query_text: &str,
    limit: usize,
) -> Result<Vec<(String, f64)>> {
    let query_text = sanitize_fts_query(query_text);
    if query_text.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT memory_id, rank FROM fts_memories WHERE fts_memories MATCH ?1 ORDER BY rank LIMIT ?2",
    )?;

    let results: Vec<(String, f64)> = stmt
        .query_map(params![query_text, limit as i64], |row| {
            let id: String = row.get(0)?;
            let rank: f64 = row.get(1)?;
            Ok((id, rank.abs()))
        })?
        .collect::<rusqlite::Result<_>>()?;

    Ok(results)
}

/// Walk up to `hops` steps out from `seed_ids`, returning each newly-found
/// neighbor with the hop depth it was found at (1 = direct neighbor of a
/// seed). `Relation` edges — grounded LLM-labeled edges with a predicate —
/// are preferred over plain similarity/temporal/tag-overlap edges when a
/// hop's frontier is larger than what fits in `limit`, since they carry
/// actual semantic meaning useful for conceptual multi-hop queries.
fn graph_expand(
    conn: &rusqlite::Connection,
    seed_ids: &[String],
    limit: usize,
    hops: usize,
) -> Result<Vec<(String, usize)>> {
    let mut visited: std::collections::HashSet<String> = seed_ids.iter().cloned().collect();
    let mut result: Vec<(String, usize)> = Vec::new();
    let mut frontier: Vec<String> = seed_ids.to_vec();

    for hop in 1..=hops.max(1) {
        if frontier.is_empty() || result.len() >= limit {
            break;
        }

        let mut candidates: Vec<(String, f64)> = Vec::new();
        for id in &frontier {
            let mut stmt = conn.prepare(
                "SELECT dst_id, edge_type, weight FROM edges WHERE src_id = ?1
                 UNION
                 SELECT src_id, edge_type, weight FROM edges WHERE dst_id = ?1",
            )?;
            let rows: Vec<(String, String, f64)> = stmt
                .query_map(params![id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .filter_map(|r| r.ok())
                .collect();

            for (nid, edge_type, weight) in rows {
                if visited.contains(&nid) {
                    continue;
                }
                // `Relation` edges get a priority boost over similarity/
                // temporal/tag-overlap when ranking which neighbors of an
                // oversized frontier make the cut.
                let priority = if edge_type == "relation" { weight + 1.0 } else { weight };
                candidates.push((nid, priority));
            }
        }

        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut next_frontier = Vec::new();
        for (nid, _) in candidates {
            if visited.insert(nid.clone()) {
                next_frontier.push(nid.clone());
                result.push((nid, hop));
                if result.len() >= limit {
                    break;
                }
            }
        }
        frontier = next_frontier;
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// RRF + scoring
// ---------------------------------------------------------------------------

fn compute_rrf(
    dense: &[(String, f64)],
    sparse: &[(String, f64)],
    graph: &[(String, usize)],
    weights: &RetrievalConfig,
) -> HashMap<String, f64> {
    let mut scores: HashMap<String, f64> = HashMap::new();

    // Dense: distance → rank (lower distance = higher rank, but we assign by order)
    for (rank, (id, _)) in dense.iter().enumerate() {
        *scores.entry(id.clone()).or_default() +=
            weights.dense_weight / (RRF_K + rank as f64 + 1.0);
    }

    // Sparse: rank by position (lower FTS rank = better)
    for (rank, (id, _)) in sparse.iter().enumerate() {
        *scores.entry(id.clone()).or_default() +=
            weights.sparse_weight / (RRF_K + rank as f64 + 1.0);
    }

    // Graph neighbors: treated as a separate "path" at higher rank positions,
    // additionally down-weighted per hop (1-hop = decay^1, 2-hop = decay^2, ...).
    let graph_base = dense.len().max(sparse.len()) as f64;
    for (rank, (id, hop)) in graph.iter().enumerate() {
        let hop_decay = GRAPH_HOP_DECAY.powi(*hop as i32);
        *scores.entry(id.clone()).or_default() +=
            weights.graph_weight * hop_decay / (RRF_K + graph_base + rank as f64 + 1.0);
    }

    scores
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Hybrid recall. `query_vec: None` skips the dense path entirely (e.g. when
/// no embedding model is available) instead of polluting RRF with a zero-vector.
pub fn recall(
    conn: &rusqlite::Connection,
    query_vec: Option<&[f32]>,
    query_text: &str,
    filters: &RecallFilters,
    limit: usize,
    weights: &RetrievalConfig,
) -> Result<Vec<RecallResult>> {
    let dense = match query_vec {
        Some(v) => dense_recall(conn, v, limit)?,
        None => Vec::new(),
    };
    let sparse = sparse_recall(conn, query_text, limit)?;

    // Seed IDs for graph expansion (union of top dense + sparse)
    let mut seed_ids: Vec<String> = Vec::new();
    for (id, _) in dense.iter().take(limit / 2) {
        if !seed_ids.contains(id) {
            seed_ids.push(id.clone());
        }
    }
    for (id, _) in sparse.iter().take(limit / 2) {
        if !seed_ids.contains(id) {
            seed_ids.push(id.clone());
        }
    }

    let graph_neighbors = graph_expand(conn, &seed_ids, limit, weights.graph_hops)?;

    // RRF fusion
    let rrf_scores = compute_rrf(&dense, &sparse, &graph_neighbors, weights);

    // Collect candidate IDs into an over-fetched pool: importance/recency/
    // strength (final_score, below) can only re-rank within this pool, so
    // truncating to `limit` here — before that scoring runs — would let a
    // high-importance item below the raw RRF cutoff get dropped before it
    // ever has a chance to be boosted back up. Keep `limit * overfetch_factor`
    // candidates, score them all, then cut to `limit`.
    let mut all_ids: Vec<String> = rrf_scores.keys().cloned().collect();
    all_ids.sort_by(|a, b| {
        rrf_scores[b]
            .partial_cmp(&rrf_scores[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    all_ids.truncate(limit.saturating_mul(weights.overfetch_factor.max(1)));

    if all_ids.is_empty() {
        return Ok(Vec::new());
    }

    // Fetch full memories and apply filters
    let placeholders: Vec<String> = all_ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 1))
        .collect();
    let (filter_where, filter_vals) = build_filter_where(filters);

    let mut sql = format!(
        "SELECT {MEMORY_COLS} FROM memories WHERE id IN ({}) {}",
        placeholders.join(", "),
        filter_where,
    );

    if filters.tag.is_some() {
        sql = sql.replace(
            "FROM memories",
            "FROM memories, json_each(metadata, '$.tags')",
        );
    }

    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<Box<dyn rusqlite::types::ToSql>> = all_ids
        .iter()
        .map(|id| Box::new(id.clone()) as Box<dyn rusqlite::types::ToSql>)
        .chain(
            filter_vals
                .into_iter()
                .map(|v| Box::new(v) as Box<dyn rusqlite::types::ToSql>),
        )
        .collect();

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let memories: Vec<Memory> = stmt
        .query_map(param_refs.as_slice(), row_to_memory)?
        .collect::<rusqlite::Result<_>>()?;

    // Build final results
    let memory_map: HashMap<String, Memory> =
        memories.into_iter().map(|m| (m.id.clone(), m)).collect();

    let mut results: Vec<RecallResult> = Vec::new();
    for id in &all_ids {
        if let Some(memory) = memory_map.get(id) {
            let rrf_score = rrf_scores[id];
            let score = final_score(rrf_score, memory.importance, memory.created_at, memory.strength);
            results.push(RecallResult {
                memory: memory.clone(),
                score,
            });
        }
    }

    // Sort by final score descending
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Update access stats
    let now = Utc::now().to_rfc3339();
    for rr in &results {
        conn.execute(
            "UPDATE memories SET accessed_at = ?1, access_count = access_count + 1 WHERE id = ?2",
            params![now, rr.memory.id],
        )?;
    }

    Ok(results)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EdgeType;

    #[test]
    fn rrf_basic_fusion() {
        let dense = vec![
            ("a".to_string(), 0.1),
            ("b".to_string(), 0.2),
            ("c".to_string(), 0.3),
        ];
        let sparse = vec![
            ("b".to_string(), 0.5),
            ("c".to_string(), 0.6),
            ("d".to_string(), 0.7),
        ];
        let graph = vec![("e".to_string(), 1)];

        let scores = compute_rrf(&dense, &sparse, &graph, &RetrievalConfig::default());

        // "b" appears in both dense rank 1 and sparse rank 0 → higher than any single-source ID
        let b_score = scores["b"];
        let a_score = scores["a"];
        let d_score = scores["d"];
        let e_score = scores["e"];

        assert!(b_score > a_score, "b should outrank a");
        assert!(b_score > d_score, "b should outrank d");
        assert!(e_score > 0.0, "graph neighbor should have non-zero score");
    }

    #[test]
    fn rrf_single_path() {
        let dense = vec![("x".to_string(), 0.1)];
        let sparse: Vec<(String, f64)> = vec![];
        let graph: Vec<(String, usize)> = vec![];
        let uniform = RetrievalConfig {
            dense_weight: 1.0,
            sparse_weight: 1.0,
            graph_weight: 1.0,
            ..RetrievalConfig::default()
        };

        let scores = compute_rrf(&dense, &sparse, &graph, &uniform);
        assert!((scores["x"] - 1.0 / (RRF_K + 1.0)).abs() < 1e-10);
    }

    #[test]
    fn zero_weight_drops_stream() {
        let dense = vec![("x".to_string(), 0.1)];
        let sparse = vec![("y".to_string(), 0.1)];
        let graph: Vec<(String, usize)> = vec![];
        let dense_off = RetrievalConfig {
            dense_weight: 0.0,
            sparse_weight: 1.0,
            graph_weight: 1.0,
            ..RetrievalConfig::default()
        };

        let scores = compute_rrf(&dense, &sparse, &graph, &dense_off);
        assert_eq!(scores["x"], 0.0, "dense_weight=0 should zero out the dense contribution");
        assert!(scores["y"] > 0.0, "sparse contribution should be unaffected");
    }

    #[test]
    fn overfetch_lets_importance_reorder_below_raw_cutoff() {
        // Without the overfetch fix, an item ranked below `limit` by raw RRF
        // could never be reached by importance/recency scoring. With
        // overfetch_factor >= 2 and limit=1, both items survive into the pool.
        let weights = RetrievalConfig {
            overfetch_factor: 2,
            ..RetrievalConfig::default()
        };
        let limit: usize = 1;
        let pool_size = limit.saturating_mul(weights.overfetch_factor.max(1));
        assert_eq!(pool_size, 2, "pool must hold more than `limit` candidates");
    }

    #[test]
    fn graph_expand_walks_multiple_hops() {
        use crate::store::Store;
        let store = Store::open_in_memory().unwrap();
        let a = store.create_memory("a", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        let b = store.create_memory("b", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        let c = store.create_memory("c", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        store.create_edge(&a.id, &b.id, EdgeType::Similarity, None, 0.9).unwrap();
        store.create_edge(&b.id, &c.id, EdgeType::Similarity, None, 0.9).unwrap();

        // hops=1: only b (a's direct neighbor) should be reachable.
        let one_hop = graph_expand(&store.conn, std::slice::from_ref(&a.id), 10, 1).unwrap();
        assert_eq!(one_hop, vec![(b.id.clone(), 1)]);

        // hops=2: c becomes reachable through b, at hop depth 2.
        let two_hop = graph_expand(&store.conn, std::slice::from_ref(&a.id), 10, 2).unwrap();
        assert!(two_hop.contains(&(b.id.clone(), 1)));
        assert!(two_hop.contains(&(c.id.clone(), 2)));
    }

    #[test]
    fn graph_expand_prefers_relation_edges_under_limit() {
        use crate::store::Store;
        let store = Store::open_in_memory().unwrap();
        let seed = store.create_memory("seed", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        let plain = store.create_memory("plain neighbor", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        let related = store.create_memory("related neighbor", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();

        // Similarity edge has a higher raw weight than the Relation edge,
        // but Relation should still win the single open slot.
        store.create_edge(&seed.id, &plain.id, EdgeType::Similarity, None, 0.95).unwrap();
        store.create_edge(&seed.id, &related.id, EdgeType::Relation, Some("depends_on"), 0.5).unwrap();

        let result = graph_expand(&store.conn, std::slice::from_ref(&seed.id), 1, 1).unwrap();
        assert_eq!(result, vec![(related.id.clone(), 1)], "Relation edge must be preferred over a higher-weight Similarity edge");
    }

    #[test]
    fn recency_boost_recent() {
        let now = Utc::now();
        let factor = recency_factor(now);
        assert!((factor - 1.0).abs() < 0.01);
    }

    #[test]
    fn recency_boost_old() {
        let old = Utc::now() - chrono::Duration::days(60);
        let factor = recency_factor(old);
        assert!((factor - 0.0).abs() < 0.01);
    }

    #[test]
    fn final_score_calculation() {
        let now = Utc::now();
        let score = final_score(0.1, 0.8, now, 1.0);
        let expected = 0.1 * (1.0 + 0.08 + 0.05) * 1.0;
        assert!((score - expected).abs() < 0.001);
    }

    #[test]
    fn strength_factor_penalizes_weak() {
        let strong = strength_factor(1.0);
        let weak = strength_factor(0.2);
        assert!(strong > weak);
        assert!((strong - 1.0).abs() < 0.01);
    }
}

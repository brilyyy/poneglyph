//! Memory consolidation — clustering similar memories into schema decoys.
//!
//! Agglomerative clustering by embedding similarity, then LLM or extractive
//! summarization to create decoy memories that represent clusters.

use anyhow::Result;
use rusqlite::params;
use std::collections::HashMap;
use tracing::{info, warn};

use crate::config::Config;
use crate::cold;
use crate::model::{Memory, MemoryType};
use crate::store::Store;

// ---------------------------------------------------------------------------
// Clustering
// ---------------------------------------------------------------------------

/// Cosine similarity between two vectors.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| (*x as f64) * (*y as f64)).sum();
    let norm_a: f64 = a.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    let norm_b: f64 = b.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// Simple agglomerative clustering: merge closest pair while similarity >= threshold.
/// Returns cluster assignments (cluster_id → list of memory indices).
fn cluster_memories(
    embeddings: &[(String, Vec<f32>)],
    threshold: f64,
    min_cluster_size: usize,
) -> Vec<Vec<usize>> {
    if embeddings.len() < min_cluster_size {
        return Vec::new();
    }

    let n = embeddings.len();
    // Union-Find for clustering
    let mut parent: Vec<usize> = (0..n).collect();
    let mut rank: Vec<usize> = vec![0; n];

    fn find(parent: &mut [usize], x: usize) -> usize {
        if parent[x] != x {
            parent[x] = find(parent, parent[x]);
        }
        parent[x]
    }

    fn union(parent: &mut [usize], rank: &mut [usize], x: usize, y: usize) {
        let rx = find(parent, x);
        let ry = find(parent, y);
        if rx == ry {
            return;
        }
        if rank[rx] < rank[ry] {
            parent[rx] = ry;
        } else if rank[rx] > rank[ry] {
            parent[ry] = rx;
        } else {
            parent[ry] = rx;
            rank[rx] += 1;
        }
    }

    // Build sorted edge list (pairs with similarity >= threshold)
    let mut edges: Vec<(f64, usize, usize)> = Vec::new();
    for i in 0..n {
        for j in (i + 1)..n {
            let sim = cosine_similarity(&embeddings[i].1, &embeddings[j].1);
            if sim >= threshold {
                edges.push((sim, i, j));
            }
        }
    }
    edges.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    // Greedy merge (single-linkage style)
    for (_sim, i, j) in edges {
        union(&mut parent, &mut rank, i, j);
    }

    // Group by cluster
    let mut clusters: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        clusters.entry(root).or_default().push(i);
    }

    // Filter by min_cluster_size
    clusters
        .into_values()
        .filter(|c| c.len() >= min_cluster_size)
        .collect()
}

// ---------------------------------------------------------------------------
// Summarization (extractive fallback)
// ---------------------------------------------------------------------------

/// Extractive summarization: pick the highest-importance memory content.
/// Fallback when no LLM is available.
fn extractive_summary(memories: &[&Memory]) -> String {
    let best = memories
        .iter()
        .max_by(|a, b| a.importance.partial_cmp(&b.importance).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap();

    // Truncate to first 280 chars if too long
    let content = best.content.trim();
    if content.len() <= 280 {
        content.to_string()
    } else {
        format!("{}...", &content[..277])
    }
}

// ---------------------------------------------------------------------------
// LLM summarization (optional)
// ---------------------------------------------------------------------------

/// Generate a summary via LLM. Returns None if LLM is not configured or fails.
async fn llm_summary(memories: &[&Memory], llm: &crate::llm::LlmClient) -> Option<String> {
    let mut prompt = String::from("Summarize these related memories into 2-3 concise sentences:\n\n");
    for (i, m) in memories.iter().enumerate() {
        prompt.push_str(&format!("{}. [{}] {}\n", i + 1, m.memory_type, m.content.chars().take(200).collect::<String>()));
    }

    match llm.complete(
        "You compress developer memory clusters into concise summaries. Preserve key facts, decisions, and context. 2-3 sentences max.",
        &prompt,
    ).await {
        Ok(summary) if !summary.trim().is_empty() => Some(summary),
        Ok(_) => {
            warn!("LLM returned empty summary, falling back to extractive");
            None
        }
        Err(e) => {
            warn!(error = %e, "LLM summarization failed, falling back to extractive");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Consolidation result for reporting.
#[derive(Debug, Clone)]
pub struct ConsolidationResult {
    pub decoy_id: String,
    pub child_count: usize,
    pub summary: String,
    /// Mean intra-cluster cosine similarity — confidence proxy for the decoy.
    pub confidence: f64,
}

/// Mean pairwise cosine similarity within a cluster — used as a confidence
/// proxy for the decoy it produces (tight clusters → higher confidence).
fn cluster_cohesion(embeddings: &[&Vec<f32>]) -> f64 {
    if embeddings.len() < 2 {
        return 1.0;
    }
    let mut sum = 0.0;
    let mut count = 0;
    for i in 0..embeddings.len() {
        for j in (i + 1)..embeddings.len() {
            sum += cosine_similarity(embeddings[i], embeddings[j]);
            count += 1;
        }
    }
    if count == 0 { 1.0 } else { sum / count as f64 }
}

/// Cluster a candidate set by embedding similarity and summarize each
/// cluster into a decoy of `decoy_type` (LLM summary, extractive fallback).
/// Generalized over the decoy's resulting `MemoryType` so this one routine
/// backs both "episodic → semantic fact" and "tool-use → procedural
/// workflow" consolidation, not just the general per-project sweep.
pub async fn consolidate_memories(
    store: &mut Store,
    candidates: &[Memory],
    decoy_type: MemoryType,
    project_id: Option<&str>,
    config: &Config,
    embedder: Option<&crate::embed::Embedder>,
) -> Result<Vec<ConsolidationResult>> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    // Fetch embeddings for candidates
    let mut embeddings: Vec<(String, Vec<f32>)> = Vec::new();
    for mem in candidates {
        let result = store.conn.query_row(
            "SELECT embedding FROM vec_memories WHERE memory_id = ?1",
            params![mem.id],
            |row| row.get::<_, Vec<u8>>(0),
        );
        if let Ok(bytes) = result {
            let vec: Vec<f32> = bytes.chunks(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect();
            if vec.len() == config.embedding.dimensions {
                embeddings.push((mem.id.clone(), vec));
            }
        }
    }

    let cluster_indices = cluster_memories(
        &embeddings,
        config.consolidation.similarity_threshold,
        config.consolidation.min_cluster_size,
    );

    if cluster_indices.is_empty() {
        return Ok(Vec::new());
    }

    // Indices from `cluster_memories` are positions in `embeddings`, which
    // is filtered down from `candidates` (skips anything with no stored
    // vector or a dimension mismatch) — so it's shorter than `candidates`
    // whenever any candidate lacks an embedding. Indexing `candidates`
    // directly by that position would silently grab the wrong memory once
    // the two lists diverge. Look up by id instead.
    let by_id: std::collections::HashMap<&str, &Memory> =
        candidates.iter().map(|m| (m.id.as_str(), m)).collect();

    let llm_client = crate::llm::LlmClient::from_config(&config.llm);
    let mut results = Vec::new();

    for indices in &cluster_indices {
        let cluster_memories: Vec<&Memory> = indices
            .iter()
            .filter_map(|&i| embeddings.get(i))
            .filter_map(|(id, _)| by_id.get(id.as_str()).copied())
            .collect();

        if cluster_memories.is_empty() {
            continue;
        }

        let summary = if let Some(ref llm) = llm_client {
            llm_summary(&cluster_memories, llm).await
                .unwrap_or_else(|| extractive_summary(&cluster_memories))
        } else {
            extractive_summary(&cluster_memories)
        };

        let max_importance = cluster_memories
            .iter()
            .map(|m| m.importance)
            .fold(0.0f64, f64::max);

        let cluster_embeddings: Vec<&Vec<f32>> = indices
            .iter()
            .filter_map(|&i| embeddings.get(i).map(|(_, v)| v))
            .collect();
        let confidence = cluster_cohesion(&cluster_embeddings);

        let decoy = store.create_decoy(
            &summary,
            decoy_type.clone(),
            max_importance,
            project_id,
            Some(&serde_json::json!({
                "child_count": cluster_memories.len(),
                "consolidation": "embedding_cluster",
                "confidence": confidence,
                "child_ids": cluster_memories.iter().map(|m| &m.id).collect::<Vec<_>>(),
            })),
        )?;

        for mem in &cluster_memories {
            store.link_decoy_child(&decoy.id, &mem.id)?;
            store.mark_consolidated(&mem.id)?;

            if let Some(pid) = project_id
                && mem.strength < config.decay.consolidation_threshold
            {
                match cold::compress_to_file(
                    &mem.content,
                    pid,
                    &mem.id,
                    config.cold_storage.compress_level,
                ) {
                    Ok(cold_path) => {
                        store.move_to_cold(&mem.id, &cold_path.to_string_lossy())?;
                    }
                    Err(e) => {
                        warn!(memory_id = %mem.id, error = %e, "failed to compress to cold storage");
                    }
                }
            }
        }

        if let Some(embedder) = embedder {
            match embedder.embed_passage(&summary).await {
                Ok(vec) => {
                    store.index_embedding(&decoy.id, &vec)?;
                    store.index_fts(&decoy.id, &summary)?;
                }
                Err(e) => {
                    warn!(error = %e, "failed to embed decoy, using FTS only");
                    store.index_fts(&decoy.id, &summary)?;
                }
            }
        } else {
            store.index_fts(&decoy.id, &summary)?;
        }

        info!(
            decoy_id = %decoy.id,
            decoy_type = %decoy_type,
            child_count = cluster_memories.len(),
            confidence,
            "consolidation complete"
        );

        results.push(ConsolidationResult {
            decoy_id: decoy.id,
            child_count: cluster_memories.len(),
            summary,
            confidence,
        });
    }

    Ok(results)
}

/// Run consolidation for a project: cluster → summarize → create decoys.
/// Always produces `Semantic` decoys — the general per-project sweep used by
/// `poneglyph consolidate` and the scheduler. See `consolidate_memories` for
/// the lower-level routine other pipeline stages reuse with a different
/// candidate set / decoy type.
pub async fn consolidate_project(
    store: &mut Store,
    project_id: &str,
    config: &Config,
    embedder: Option<&crate::embed::Embedder>,
) -> Result<Vec<ConsolidationResult>> {
    let candidates = store.get_consolidation_candidates(
        project_id,
        config.decay.consolidation_threshold,
    )?;

    if candidates.is_empty() {
        info!(project_id, "no consolidation candidates");
        return Ok(Vec::new());
    }

    info!(project_id, count = candidates.len(), "consolidation candidates found");

    let results = consolidate_memories(
        store,
        &candidates,
        MemoryType::Semantic,
        Some(project_id),
        config,
        embedder,
    )
    .await?;

    if results.is_empty() {
        info!(project_id, "no clusters found above threshold");
    }

    Ok(results)
}

/// Run decay update: recompute strengths, archive low-strength memories.
pub fn run_decay(store: &Store, config: &Config) -> Result<DecayReport> {
    if !config.decay.enabled {
        return Ok(DecayReport::default());
    }

    // Update all strengths
    let updated = store.update_all_strengths()?;

    // Archive low-strength memories to cold
    let candidates = store.get_cold_candidates(
        config.decay.min_strength,
        14, // at least 14 days old
    )?;

    let mut archived = 0;
    let mut pruned = 0;

    for mem in &candidates {
        // If it's a child of a decoy, compress to cold
        if store.get_child_decoy(&mem.id)?.is_some() {
            if let Ok(cold_path) = cold::compress_to_file(
                &mem.content,
                &mem.project_id.as_deref().unwrap_or("unknown"),
                &mem.id,
                config.cold_storage.compress_level,
            ) {
                store.move_to_cold(&mem.id, &cold_path.to_string_lossy())?;
                archived += 1;
            }
        }
        // If strength is very low and not consolidated, consider pruning
        else if mem.strength < config.decay.min_strength * 0.5 && mem.access_count == 0 {
            // Create a cold backup first
            if let Some(ref project_id) = mem.project_id {
                if let Ok(cold_path) = cold::compress_to_file(
                    &mem.content,
                    project_id,
                    &mem.id,
                    config.cold_storage.compress_level,
                ) {
                    store.move_to_cold(&mem.id, &cold_path.to_string_lossy())?;
                    pruned += 1;
                }
            }
        }
    }

    info!(
        strengths_updated = updated,
        archived,
        pruned,
        "decay run complete"
    );

    Ok(DecayReport {
        strengths_updated: updated,
        archived,
        pruned,
    })
}

#[derive(Debug, Clone, Default)]
pub struct DecayReport {
    pub strengths_updated: i64,
    pub archived: usize,
    pub pruned: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MemoryType, Source, Tier};
    use crate::store::Store;

    /// Regression for a bug found via the pipeline tests: when a candidate
    /// set mixes memories with and without a stored embedding, the
    /// `embeddings` vec is shorter than `candidates` (unembedded ones are
    /// skipped), so cluster indices — positions in `embeddings` — must be
    /// resolved back to memories by id, not by indexing `candidates`
    /// directly. Indexing `candidates` instead would silently attribute
    /// the cluster to the wrong memory.
    #[tokio::test]
    async fn consolidate_memories_handles_unembedded_candidate_mixed_in() {
        let mut store = Store::open_in_memory().unwrap();
        let project = store.upsert_project("/p", "p", None).unwrap();
        let config = Config::default();

        // No embedding at all — must never end up in the resulting decoy's
        // children even though it shares the candidate list.
        let unembedded = store
            .create_memory("no vector", MemoryType::Episodic, 0.5, Source::Cli, Some(&project.id), None)
            .unwrap();
        let a = store
            .create_memory("a", MemoryType::Episodic, 0.5, Source::Cli, Some(&project.id), None)
            .unwrap();
        store.index_embedding(&a.id, &vec![1.0_f32; config.embedding.dimensions]).unwrap();
        let b = store
            .create_memory("b", MemoryType::Episodic, 0.5, Source::Cli, Some(&project.id), None)
            .unwrap();
        store.index_embedding(&b.id, &vec![1.0_f32; config.embedding.dimensions]).unwrap();

        // Candidate order matters for the regression: the unembedded one
        // sits between the two that do have vectors.
        let candidates = vec![a.clone(), unembedded.clone(), b.clone()];

        let results = consolidate_memories(&mut store, &candidates, MemoryType::Semantic, Some(&project.id), &config, None)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        let children = store.get_decoy_children(&results[0].decoy_id).unwrap();
        let child_ids: Vec<&str> = children.iter().map(|m| m.id.as_str()).collect();
        assert!(child_ids.contains(&a.id.as_str()));
        assert!(child_ids.contains(&b.id.as_str()));
        assert!(!child_ids.contains(&unembedded.id.as_str()), "unembedded candidate must never be attributed to the cluster");
    }

    #[test]
    fn cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!((cosine_similarity(&a, &b) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_different_lengths() {
        let a = vec![1.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn cluster_small_group() {
        let embeddings = vec![
            ("a".to_string(), vec![1.0, 0.0, 0.0]),
            ("b".to_string(), vec![0.9, 0.1, 0.0]),
            ("c".to_string(), vec![0.0, 0.0, 1.0]),
        ];
        let clusters = cluster_memories(&embeddings, 0.7, 2);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].len(), 2);
    }

    #[test]
    fn cluster_below_min_size() {
        let embeddings = vec![
            ("a".to_string(), vec![1.0, 0.0]),
            ("b".to_string(), vec![0.0, 1.0]),
        ];
        let clusters = cluster_memories(&embeddings, 0.7, 3);
        assert!(clusters.is_empty());
    }

    #[test]
    fn extractive_summary_picks_best() {
        let m1 = Memory {
            id: "1".to_string(),
            content: "low importance note".to_string(),
            memory_type: MemoryType::Fact,
            importance: 0.2,
            project_id: None,
            source: Source::Cli,
            metadata: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            accessed_at: None,
            access_count: 0,
            is_decoy: false,
            tier: Tier::Hot,
            strength: 1.0,
            cold_path: None,
        };
        let m2 = Memory {
            id: "2".to_string(),
            content: "critical decision".to_string(),
            memory_type: MemoryType::Semantic,
            importance: 0.9,
            project_id: None,
            source: Source::Cli,
            metadata: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            accessed_at: None,
            access_count: 0,
            is_decoy: false,
            tier: Tier::Hot,
            strength: 1.0,
            cold_path: None,
        };
        let summary = extractive_summary(&[&m1, &m2]);
        assert_eq!(summary, "critical decision");
    }
}

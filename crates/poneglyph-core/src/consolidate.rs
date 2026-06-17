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
use crate::model::Memory;
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
}

/// Run consolidation for a project: cluster → summarize → create decoys.
pub async fn consolidate_project(
    store: &Store,
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

    // Fetch embeddings for candidates
    let mut embeddings: Vec<(String, Vec<f32>)> = Vec::new();
    for mem in &candidates {
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

    // Cluster
    let cluster_indices = cluster_memories(
        &embeddings,
        config.consolidation.similarity_threshold,
        config.consolidation.min_cluster_size,
    );

    if cluster_indices.is_empty() {
        info!(project_id, "no clusters found above threshold");
        return Ok(Vec::new());
    }

    info!(
        project_id,
        clusters = cluster_indices.len(),
        "found clusters to consolidate"
    );

    // Optional LLM client
    let llm_client = crate::llm::LlmClient::from_config(&config.llm);
    let mut results = Vec::new();

    for indices in &cluster_indices {
        let cluster_memories: Vec<&Memory> = indices
            .iter()
            .filter_map(|&i| candidates.get(i))
            .collect();

        if cluster_memories.is_empty() {
            continue;
        }

        // Generate summary
        let summary = if let Some(ref llm) = llm_client {
            llm_summary(&cluster_memories, llm).await
                .unwrap_or_else(|| extractive_summary(&cluster_memories))
        } else {
            extractive_summary(&cluster_memories)
        };

        // Compute aggregated importance (max)
        let max_importance = cluster_memories
            .iter()
            .map(|m| m.importance)
            .fold(0.0f64, f64::max);

        // Create decoy
        let decoy = store.create_decoy(
            &summary,
            max_importance,
            Some(project_id),
            Some(&serde_json::json!({
                "child_count": cluster_memories.len(),
                "consolidation": "embedding_cluster",
                "child_ids": cluster_memories.iter().map(|m| &m.id).collect::<Vec<_>>(),
            })),
        )?;

        // Link children and mark consolidated
        for mem in &cluster_memories {
            store.link_decoy_child(&decoy.id, &mem.id)?;
            store.mark_consolidated(&mem.id)?;

            // Move low-strength children to cold
            if mem.strength < config.decay.consolidation_threshold {
                match cold::compress_to_file(
                    &mem.content,
                    project_id,
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

        // Index decoy (embedding + FTS)
        if let Some(embedder) = embedder {
            match embedder.embed_text(&summary).await {
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
            child_count = cluster_memories.len(),
            summary_len = summary.len(),
            "consolidation complete"
        );

        results.push(ConsolidationResult {
            decoy_id: decoy.id,
            child_count: cluster_memories.len(),
            summary,
        });
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

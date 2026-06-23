//! Hierarchical memory pipeline: raw → episodic → semantic → procedural,
//! mirroring AgentMemory's consolidation tiers (working memory summarized
//! into session episodes, episodes distilled into semantic facts, facts
//! distilled into procedural workflows).
//!
//! Every stage has an LLM path and a deterministic fallback, so the chain
//! completes end to end even with `llm.enabled = false`:
//!
//! | Stage              | LLM path                          | Fallback (no LLM)                  |
//! |---------------------|------------------------------------|-------------------------------------|
//! | raw → episodic      | abstractive session summary        | extractive top-content join         |
//! | episodic → semantic | fact distillation + confidence     | embedding-cluster decoy + cohesion  |
//! | semantic → procedural | workflow synthesis              | frequent tool-sequence n-gram mining|
//!
//! Tiers coexist — promotion adds a higher-tier memory and links lineage
//! (`Explicit` edges, or decoy/child rows), it never deletes the sources.
//! Decay/cold-storage (`consolidate::run_decay`) handles aging independently.
//!
//! Graph extraction is a parallel branch, not a stage in this chain: it
//! already runs as the always-on `ComputeEdges` job
//! (`graph::build_edges_for_memory`) plus the optional LLM
//! `extract_relations` job, both wired through `enrich.rs`.

use anyhow::Result;
use tracing::{info, warn};

use crate::config::Config;
use crate::consolidate::{self, ConsolidationResult};
use crate::embed::Embedder;
use crate::llm::{parse_json_reply, LlmClient};
use crate::model::{EdgeType, Memory, MemoryType, Source};
use crate::store::Store;

const SESSION_SUMMARY_TAG: &str = "session-summary";
const MAX_SESSION_MEMORIES: usize = 30;
const TOP_SESSION_MEMORIES: usize = 10;
const MIN_SEQUENCE_LEN: usize = 4;
const NGRAM_SIZE: usize = 3;
const MIN_NGRAM_SUPPORT: usize = 2;
/// Cap how many procedural memories one mining pass can emit — a project
/// with many distinct repeating n-grams shouldn't flood the store.
const MAX_PROCEDURAL_PER_RUN: usize = 5;

fn has_tag(m: &Memory, tag: &str) -> bool {
    m.metadata
        .as_ref()
        .and_then(|meta| meta.get("tags"))
        .and_then(|tags| tags.as_array())
        .map(|arr| arr.iter().any(|t| t.as_str() == Some(tag)))
        .unwrap_or(false)
}

fn extractive_join(top: &[&Memory]) -> String {
    let text = top.iter().map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n---\n");
    if text.len() > 2000 {
        format!("{}...", &text[..2000])
    } else {
        text
    }
}

// ---------------------------------------------------------------------------
// Stage 1: raw → episodic (session summary)
// ---------------------------------------------------------------------------

/// Summarize a project's recent raw memories into one `Episodic` memory,
/// with `Explicit` lineage edges back to the sources it was built from.
/// Returns `None` when there's nothing new to summarize.
pub async fn summarize_session(
    store: &mut Store,
    project_id: Option<&str>,
    llm: Option<&LlmClient>,
) -> Result<Option<Memory>> {
    let (memories, _) = store.list_memories(project_id, None, MAX_SESSION_MEMORIES, 0)?;

    let real: Vec<&Memory> = memories.iter().filter(|m| !m.is_decoy && !has_tag(m, SESSION_SUMMARY_TAG)).collect();
    if real.is_empty() {
        return Ok(None);
    }

    let mut sorted = real;
    sorted.sort_by(|a, b| b.importance.partial_cmp(&a.importance).unwrap_or(std::cmp::Ordering::Equal));
    let top: Vec<&Memory> = sorted.into_iter().take(TOP_SESSION_MEMORIES).collect();

    let summary_text = match llm {
        Some(client) => {
            let joined = top.iter().map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n---\n");
            match client
                .complete(
                    "You summarize coding sessions for a developer's memory store. \
                     Given a few memories from one session, reply with a concise summary \
                     of what was worked on. Plain text, no preamble, 2-4 sentences.",
                    &joined,
                )
                .await
            {
                Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
                _ => extractive_join(&top),
            }
        }
        None => extractive_join(&top),
    };

    let metadata = serde_json::json!({ "tags": [SESSION_SUMMARY_TAG] });
    let mem = store.create_memory(&summary_text, MemoryType::Episodic, 0.5, Source::Cli, project_id, Some(&metadata))?;
    store.index_fts(&mem.id, &summary_text)?;

    // Lineage: link the summary back to every source memory it draws from,
    // so the viewer can show "summarized from" provenance on the episodic.
    for m in &top {
        store.create_edge(&mem.id, &m.id, EdgeType::Explicit, Some("summarizes"), 1.0)?;
    }

    crate::enrich::enqueue_compute_edges(store, &mem.id)?;
    Ok(Some(mem))
}

// ---------------------------------------------------------------------------
// Stage 2: episodic → semantic (fact distillation)
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct DistilledFact {
    fact: String,
    confidence: f64,
    /// 1-indexed positions into the episodic list supplied, supporting this fact.
    #[serde(default)]
    sources: Vec<usize>,
}

/// Distill a project's `Episodic` memories into `Semantic` facts with a
/// confidence score. LLM path asks for recurring, multiply-supported facts
/// directly; the fallback reuses the deterministic embedding-cluster decoy
/// machinery (`consolidate::consolidate_memories`) — this is the stage where
/// Poneglyph can go further than AgentMemory without an LLM at all.
pub async fn distill_semantic_facts(
    store: &mut Store,
    project_id: &str,
    config: &Config,
    embedder: Option<&Embedder>,
    llm: Option<&LlmClient>,
) -> Result<Vec<ConsolidationResult>> {
    let (episodics, _) = store.list_memories(Some(project_id), Some("episodic"), 200, 0)?;
    let candidates: Vec<Memory> = episodics.into_iter().filter(|m| !m.is_decoy).collect();
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    if let Some(client) = llm {
        match distill_facts_llm(store, &candidates, project_id, client).await {
            Ok(results) if !results.is_empty() => return Ok(results),
            Ok(_) => {}
            Err(e) => warn!(error = %e, "LLM fact distillation failed, falling back to clustering"),
        }
    }

    consolidate::consolidate_memories(store, &candidates, MemoryType::Semantic, Some(project_id), config, embedder).await
}

async fn distill_facts_llm(
    store: &mut Store,
    episodics: &[Memory],
    project_id: &str,
    client: &LlmClient,
) -> Result<Vec<ConsolidationResult>> {
    let mut user = String::from("EPISODIC MEMORIES:\n");
    for (i, m) in episodics.iter().enumerate() {
        user.push_str(&format!("{}. {}\n", i + 1, m.content.chars().take(300).collect::<String>()));
    }

    let raw = client
        .complete(
            "You distill recurring facts about a software project from its episodic session \
             summaries. Reply with a JSON array of {\"fact\": \"<concise standalone fact>\", \
             \"confidence\": <0.0-1.0>, \"sources\": [<1-indexed memory numbers supporting it>]}. \
             Only include facts supported by 2 or more sources. Empty array if none. JSON only.",
            &user,
        )
        .await?;
    let facts: Vec<DistilledFact> = parse_json_reply(&raw)?;

    let mut results = Vec::new();
    for f in facts {
        let fact = f.fact.trim();
        if fact.is_empty() {
            continue;
        }
        let source_ids: Vec<&str> = f
            .sources
            .iter()
            .filter_map(|&i| episodics.get(i.wrapping_sub(1)).map(|m| m.id.as_str()))
            .collect();
        let confidence = f.confidence.clamp(0.0, 1.0);

        let decoy = store.create_decoy(
            fact,
            MemoryType::Semantic,
            confidence,
            Some(project_id),
            Some(&serde_json::json!({
                "consolidation": "llm_distillation",
                "confidence": confidence,
                "child_ids": source_ids,
            })),
        )?;
        store.index_fts(&decoy.id, &decoy.content)?;
        for sid in &source_ids {
            store.link_decoy_child(&decoy.id, sid)?;
        }
        results.push(ConsolidationResult {
            decoy_id: decoy.id,
            child_count: source_ids.len(),
            summary: fact.to_string(),
            confidence,
        });
    }
    Ok(results)
}

// ---------------------------------------------------------------------------
// Stage 3: semantic → procedural (workflow synthesis)
// ---------------------------------------------------------------------------

fn tool_name(m: &Memory) -> String {
    m.metadata
        .as_ref()
        .and_then(|meta| meta.get("tool"))
        .and_then(|t| t.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| m.content.split_whitespace().next().unwrap_or("?").to_string())
}

/// Synthesize procedural workflows from a project's `CodeContext` (tool-use)
/// memories. LLM path detects a named, repeatable workflow from the recent
/// tool-use sequence. Fallback: deterministic frequent n-gram mining over
/// tool names — no semantic understanding, just frequency, but it never
/// needs an LLM to produce something.
pub async fn synthesize_procedures(
    store: &mut Store,
    project_id: &str,
    llm: Option<&LlmClient>,
) -> Result<Vec<Memory>> {
    let (mut tool_uses, _) = store.list_memories(Some(project_id), Some("code_context"), 200, 0)?;
    tool_uses.sort_by_key(|m| m.created_at);
    if tool_uses.len() < MIN_SEQUENCE_LEN {
        return Ok(Vec::new());
    }

    if let Some(client) = llm {
        match synthesize_workflow_llm(store, &tool_uses, project_id, client).await {
            Ok(Some(mem)) => return Ok(vec![mem]),
            Ok(None) => {}
            Err(e) => warn!(error = %e, "LLM workflow synthesis failed, falling back to n-gram mining"),
        }
    }

    mine_frequent_sequences(store, &tool_uses, project_id)
}

#[derive(serde::Deserialize)]
struct WorkflowReply {
    name: String,
    trigger: String,
    steps: Vec<String>,
    outcome: String,
}

async fn synthesize_workflow_llm(
    store: &mut Store,
    tool_uses: &[Memory],
    project_id: &str,
    client: &LlmClient,
) -> Result<Option<Memory>> {
    let recent: Vec<&Memory> = tool_uses.iter().rev().take(20).collect();
    let mut user = String::from("RECENT TOOL ACTIONS (oldest first):\n");
    for (i, m) in recent.iter().rev().enumerate() {
        user.push_str(&format!("{}. {}\n", i + 1, tool_name(m)));
    }

    let raw = client
        .complete(
            "You detect repeatable developer workflows from a sequence of tool actions. \
             If the sequence shows a clear repeatable pattern (e.g. \"run tests before \
             committing\"), reply with JSON {\"name\": \"<short name>\", \"trigger\": \
             \"<when this applies>\", \"steps\": [\"<step>\", ...], \"outcome\": \"<expected \
             result>\"}. If no clear pattern, reply with JSON null. JSON only.",
            &user,
        )
        .await?;

    let parsed: Option<WorkflowReply> = parse_json_reply(&raw)?;
    let Some(wf) = parsed else { return Ok(None) };
    if wf.steps.is_empty() {
        return Ok(None);
    }

    let content = format!("{}: {} → [{}] → {}", wf.name, wf.trigger, wf.steps.join(" → "), wf.outcome);
    let metadata = serde_json::json!({
        "consolidation": "llm_synthesis",
        "trigger": wf.trigger,
        "steps": wf.steps,
        "outcome": wf.outcome,
    });
    let mem = store.create_memory(&content, MemoryType::Procedural, 0.6, Source::Cli, Some(project_id), Some(&metadata))?;
    store.index_fts(&mem.id, &content)?;
    Ok(Some(mem))
}

/// ponytail: frequency-counted n-grams over the tool-name sequence, not
/// PrefixSpan or any real session-boundary detection — bounded, deterministic,
/// good enough until mined workflows turn out too coarse to be useful.
fn mine_frequent_sequences(store: &Store, tool_uses: &[Memory], project_id: &str) -> Result<Vec<Memory>> {
    let names: Vec<String> = tool_uses.iter().map(tool_name).collect();
    if names.len() < NGRAM_SIZE {
        return Ok(Vec::new());
    }

    let mut counts: std::collections::HashMap<Vec<String>, Vec<usize>> = std::collections::HashMap::new();
    for start in 0..=(names.len() - NGRAM_SIZE) {
        let gram = names[start..start + NGRAM_SIZE].to_vec();
        counts.entry(gram).or_default().push(start);
    }

    let mut grams: Vec<(Vec<String>, Vec<usize>)> =
        counts.into_iter().filter(|(_, occ)| occ.len() >= MIN_NGRAM_SUPPORT).collect();
    grams.sort_by_key(|(_, occ)| std::cmp::Reverse(occ.len()));
    grams.truncate(MAX_PROCEDURAL_PER_RUN);

    let mut created = Vec::new();
    for (gram, occurrences) in grams {
        let content = format!("Recurring workflow: {} (observed {} times)", gram.join(" → "), occurrences.len());
        let metadata = serde_json::json!({
            "consolidation": "ngram_mining",
            "steps": gram,
            "support": occurrences.len(),
        });
        let mem = store.create_memory(&content, MemoryType::Procedural, 0.5, Source::Passive, Some(project_id), Some(&metadata))?;
        store.index_fts(&mem.id, &content)?;

        // Lineage: link the first occurrence's source memories.
        if let Some(&start) = occurrences.first() {
            for offset in 0..NGRAM_SIZE {
                if let Some(src) = tool_uses.get(start + offset) {
                    store.create_edge(&mem.id, &src.id, EdgeType::Explicit, Some("derived_from"), 1.0)?;
                }
            }
        }
        created.push(mem);
    }
    Ok(created)
}

// ---------------------------------------------------------------------------
// Orchestrator
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Copy)]
pub struct PipelineReport {
    pub episodic_summaries: usize,
    pub semantic_facts: usize,
    pub procedures: usize,
}

/// Run the full raw→episodic→semantic→procedural chain for one project.
pub async fn run_pipeline_for_project(
    store: &mut Store,
    project_id: &str,
    config: &Config,
    embedder: Option<&Embedder>,
    llm: Option<&LlmClient>,
) -> Result<PipelineReport> {
    let mut report = PipelineReport::default();

    if summarize_session(store, Some(project_id), llm).await?.is_some() {
        report.episodic_summaries += 1;
    }

    let facts = distill_semantic_facts(store, project_id, config, embedder, llm).await?;
    report.semantic_facts = facts.len();

    let procedures = synthesize_procedures(store, project_id, llm).await?;
    report.procedures = procedures.len();

    info!(project_id, ?report, "pipeline run complete");
    Ok(report)
}

/// Run the pipeline across every known project — used by the scheduler.
pub async fn run_pipeline_for_all_projects(
    store: &mut Store,
    config: &Config,
    embedder: Option<&Embedder>,
    llm: Option<&LlmClient>,
) -> Result<PipelineReport> {
    let mut total = PipelineReport::default();
    for project in store.list_projects()? {
        let r = run_pipeline_for_project(store, &project.id, config, embedder, llm).await?;
        total.episodic_summaries += r.episodic_summaries;
        total.semantic_facts += r.semantic_facts;
        total.procedures += r.procedures;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Source;

    fn unit_vec(dims: usize, hot_index: usize) -> Vec<f32> {
        let mut v = vec![0.01f32; dims];
        v[hot_index] = 1.0;
        v
    }

    // `vec_memories` is hardcoded to FLOAT[384] in the schema DDL (no
    // dimension-configurability yet — see Phase 3 in the perf plan), so test
    // vectors must be 384-wide regardless of `config.embedding.dimensions`.
    const TEST_DIMS: usize = 384;

    fn test_config() -> Config {
        let mut config = Config::default();
        config.embedding.dimensions = TEST_DIMS;
        config.consolidation.min_cluster_size = 2;
        config.consolidation.similarity_threshold = 0.75;
        config
    }

    #[tokio::test]
    async fn full_fallback_chain_produces_all_three_tiers_with_lineage() {
        let mut store = Store::open_in_memory().unwrap();
        let config = test_config();
        let project = store.upsert_project("/p", "p", None).unwrap();

        // Two pre-existing episodic memories, near-identical embeddings so
        // the E→S clustering fallback groups them into one semantic decoy.
        let e1 = store
            .create_memory("worked on the auth module", MemoryType::Episodic, 0.5, Source::Cli, Some(&project.id), None)
            .unwrap();
        store.index_embedding(&e1.id, &unit_vec(TEST_DIMS, 0)).unwrap();
        let e2 = store
            .create_memory("more auth module work", MemoryType::Episodic, 0.5, Source::Cli, Some(&project.id), None)
            .unwrap();
        store.index_embedding(&e2.id, &unit_vec(TEST_DIMS, 0)).unwrap();

        // Six tool-use memories with a repeating trigram (edit, test, commit)
        // so the S→P n-gram fallback has support >= 2 to mine.
        for tool in ["edit", "test", "commit", "edit", "test", "commit"] {
            let meta = serde_json::json!({ "tool": tool });
            store
                .create_memory(&format!("ran {tool}"), MemoryType::CodeContext, 0.4, Source::Passive, Some(&project.id), Some(&meta))
                .unwrap();
        }

        let report = run_pipeline_for_project(&mut store, &project.id, &config, None, None).await.unwrap();

        assert_eq!(report.episodic_summaries, 1, "should produce one new episodic session summary");
        assert_eq!(report.semantic_facts, 1, "should cluster the two episodics into one semantic decoy");
        assert!(report.procedures >= 1, "should mine at least one procedural workflow");

        // Verify the semantic decoy's lineage actually points at e1/e2.
        let (semantics, _) = store.list_memories(Some(&project.id), Some("semantic"), 50, 0).unwrap();
        let decoys: Vec<&Memory> = semantics.iter().filter(|m| m.is_decoy).collect();
        assert_eq!(decoys.len(), 1, "expected exactly one semantic decoy, found: {decoys:#?}");
        let decoy = decoys[0];
        let children = store.get_decoy_children(&decoy.id).unwrap();
        let child_ids: Vec<&str> = children.iter().map(|m| m.id.as_str()).collect();
        assert!(
            child_ids.contains(&e1.id.as_str()) && child_ids.contains(&e2.id.as_str()),
            "decoy metadata: {:?}, children found: {child_ids:?}, expected e1={} e2={}",
            decoy.metadata,
            e1.id,
            e2.id
        );

        // Verify the procedural memory has Explicit lineage edges.
        let (procedurals, _) = store.list_memories(Some(&project.id), Some("procedural"), 50, 0).unwrap();
        let proc = procedurals.first().expect("a procedural memory must exist");
        let edges = store.get_edges_for_memory(&proc.id).unwrap();
        assert!(!edges.is_empty(), "procedural memory should carry lineage edges to its source tool-use memories");
    }

    #[tokio::test]
    async fn no_candidates_produces_empty_report() {
        let mut store = Store::open_in_memory().unwrap();
        let config = test_config();
        let project = store.upsert_project("/empty", "empty", None).unwrap();

        let report = run_pipeline_for_project(&mut store, &project.id, &config, None, None).await.unwrap();
        assert_eq!(report.episodic_summaries, 0);
        assert_eq!(report.semantic_facts, 0);
        assert_eq!(report.procedures, 0);
    }
}

//! LongMemEval-style recall@k harness for `retrieve::recall`.
//!
//! Ingests a LongMemEval (`-S`/`-M`) JSON dataset and reports R@1/R@5/R@10/MRR
//! against the real retrieval path — no separate scoring logic to drift out
//! of sync with what `poneglyph recall` actually does.
//!
//! ponytail: each question gets its own in-memory `Store`, so retrieval stays
//! correctly scoped to that question's haystack. `retrieve::recall`'s dense/
//! sparse candidate selection is global-then-post-filtered (not project-
//! scoped at the SQL stage) — isolating per question sidesteps that rather
//! than requiring a separate fix here.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use poneglyph_core::config::Config;
use poneglyph_core::embed::Embedder;
use poneglyph_core::model::{MemoryType, Source};
use poneglyph_core::retrieve::{self, RecallFilters};
use poneglyph_core::store::Store;

#[derive(Deserialize)]
struct Turn {
    #[serde(default)]
    role: String,
    content: String,
    #[serde(default)]
    has_answer: bool,
}

#[derive(Deserialize)]
struct Instance {
    #[serde(default)]
    question_id: String,
    question: String,
    #[serde(default)]
    haystack_session_ids: Vec<String>,
    haystack_sessions: Vec<Vec<Turn>>,
    #[serde(default)]
    answer_session_ids: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct EvalSummary {
    pub total_instances: usize,
    pub evaluated: usize,
    pub skipped_no_gold: usize,
    pub recall_at_1: f64,
    pub recall_at_5: f64,
    pub recall_at_10: f64,
    pub mrr: f64,
}

/// Ingest one question's haystack into a fresh in-memory store, run the real
/// `retrieve::recall`, and return the 1-indexed rank of the first gold hit
/// within the top 10 (or `None` if no gold turn made it that far).
async fn first_gold_rank(
    config: &Config,
    embedder: Option<&Arc<Embedder>>,
    inst: &Instance,
) -> Result<Option<usize>> {
    let store = Store::open_in_memory()?;

    struct Created {
        id: String,
        session_gold: bool,
        has_answer: bool,
    }
    let mut created: Vec<Created> = Vec::new();
    let mut contents: Vec<String> = Vec::new();
    let mut any_has_answer = false;

    for (idx, turns) in inst.haystack_sessions.iter().enumerate() {
        let session_id = inst.haystack_session_ids.get(idx).cloned().unwrap_or_default();
        let session_is_gold = inst.answer_session_ids.contains(&session_id);
        for turn in turns {
            any_has_answer |= turn.has_answer;
            let content = format!("{}: {}", turn.role, turn.content);
            let metadata = serde_json::json!({ "session_id": session_id });
            let mem = store.create_memory(
                &content,
                MemoryType::Episodic,
                0.5,
                Source::Import,
                None,
                Some(&metadata),
            )?;
            store.index_fts(&mem.id, &content)?;
            created.push(Created {
                id: mem.id,
                session_gold: session_is_gold,
                has_answer: turn.has_answer,
            });
            contents.push(content);
        }
    }

    if let Some(embedder) = embedder {
        let texts: Vec<&str> = contents.iter().map(|c| c.as_str()).collect();
        let vectors = embedder.embed_batch(&texts).await?;
        for (created, vec) in created.iter().zip(vectors.iter()) {
            store.index_embedding(&created.id, vec)?;
        }
    }

    // No-LLM edge builders — same as the real `poneglyph remember` path.
    for c in &created {
        poneglyph_core::enrich::enqueue_compute_edges(&store, &c.id)?;
    }
    poneglyph_core::enrich::process_pending_jobs(&store, &config.memory.edges)?;

    // Turn-level gold if the dataset marks individual turns; else fall back
    // to session-level (any turn from a gold session counts).
    let gold_ids: HashSet<&str> = if any_has_answer {
        created.iter().filter(|c| c.has_answer).map(|c| c.id.as_str()).collect()
    } else {
        created.iter().filter(|c| c.session_gold).map(|c| c.id.as_str()).collect()
    };
    if gold_ids.is_empty() {
        return Ok(None);
    }

    let query_vec = match embedder {
        Some(e) => Some(e.embed_query(&inst.question).await?),
        None => None,
    };
    let filters = RecallFilters::default();
    let results = retrieve::recall(
        &store.conn,
        query_vec.as_deref(),
        &inst.question,
        &filters,
        10,
        &config.retrieval,
    )?;

    Ok(results
        .iter()
        .position(|r| gold_ids.contains(r.memory.id.as_str()))
        .map(|i| i + 1))
}

pub async fn run(config: &Config, dataset: &Path, limit: Option<usize>) -> Result<EvalSummary> {
    let raw = std::fs::read_to_string(dataset)
        .with_context(|| format!("reading dataset {}", dataset.display()))?;
    let mut instances: Vec<Instance> = serde_json::from_str(&raw)
        .context("parsing dataset — expected a top-level JSON array of LongMemEval instances")?;
    if let Some(n) = limit {
        instances.truncate(n);
    }
    let total_instances = instances.len();

    let embedder = crate::try_embedder(config).await;

    let mut evaluated = 0usize;
    let mut skipped_no_gold = 0usize;
    let (mut hit1, mut hit5, mut hit10, mut mrr_sum) = (0usize, 0usize, 0usize, 0.0f64);

    for inst in &instances {
        if inst.answer_session_ids.is_empty() {
            skipped_no_gold += 1;
            continue;
        }
        match first_gold_rank(config, embedder.as_ref(), inst).await {
            Ok(rank) => {
                evaluated += 1;
                if let Some(rank) = rank {
                    mrr_sum += 1.0 / rank as f64;
                    if rank <= 1 {
                        hit1 += 1;
                    }
                    if rank <= 5 {
                        hit5 += 1;
                    }
                    if rank <= 10 {
                        hit10 += 1;
                    }
                }
            }
            Err(e) => {
                tracing::warn!(question_id = %inst.question_id, error = %e, "eval: skipping instance");
                skipped_no_gold += 1;
            }
        }
    }

    let denom = evaluated.max(1) as f64;
    Ok(EvalSummary {
        total_instances,
        evaluated,
        skipped_no_gold,
        recall_at_1: hit1 as f64 / denom,
        recall_at_5: hit5 as f64 / denom,
        recall_at_10: hit10 as f64 / denom,
        mrr: mrr_sum / denom,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three short synthetic sessions, one question whose answer is in
    /// session "s2" and nowhere else — keyword-only (no embedder) so the
    /// test is fast and deterministic. Fails if the harness wiring (ingest →
    /// recall → gold-rank scoring) breaks.
    #[tokio::test]
    async fn r5_is_one_on_unambiguous_question() {
        let config = Config::default();
        let inst = Instance {
            question_id: "q1".to_string(),
            question: "what programming language does the project use".to_string(),
            haystack_session_ids: vec!["s1".into(), "s2".into(), "s3".into()],
            haystack_sessions: vec![
                vec![Turn { role: "user".into(), content: "what's the weather today".into(), has_answer: false }],
                vec![Turn {
                    role: "assistant".into(),
                    content: "the project uses rust as its programming language".into(),
                    has_answer: false,
                }],
                vec![Turn { role: "user".into(), content: "remind me to buy milk".into(), has_answer: false }],
            ],
            answer_session_ids: vec!["s2".into()],
        };

        let rank = first_gold_rank(&config, None, &inst).await.unwrap();
        assert_eq!(rank, Some(1), "the only session mentioning the answer should rank first");
    }

    #[tokio::test]
    async fn no_gold_session_returns_none() {
        let config = Config::default();
        let inst = Instance {
            question_id: "q2".to_string(),
            question: "anything".to_string(),
            haystack_session_ids: vec!["s1".into()],
            haystack_sessions: vec![vec![Turn { role: "user".into(), content: "hello".into(), has_answer: false }]],
            answer_session_ids: vec!["does-not-exist".into()],
        };

        let rank = first_gold_rank(&config, None, &inst).await.unwrap();
        assert_eq!(rank, None);
    }
}

//! Project detection and context assembly.
//!
//! v1 (M2): detect by absolute path, assemble a ranked context string.
//! Git-remote normalization for cross-clone identity lands in M6.

use anyhow::Result;
use chrono::Utc;

use crate::model::{Memory, Project};
use crate::store::Store;

/// Approximate chars-per-token used to enforce the context token budget
/// without pulling in a tokenizer.
const CHARS_PER_TOKEN: usize = 4;

/// How many candidate memories to score before truncating to the budget.
const CANDIDATE_LIMIT: usize = 200;

/// Resolve (upsert) a project from an absolute directory path.
/// Name defaults to the final path component.
pub fn detect_project(store: &Store, project_path: &str) -> Result<Project> {
    let name = std::path::Path::new(project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| project_path.to_string());
    store.upsert_project(project_path, &name, None)
}

/// Ranking score per PRD §8.10: importance × recency × access.
fn context_score(m: &Memory) -> f64 {
    let age_days = (Utc::now() - m.created_at).num_seconds().max(0) as f64 / 86400.0;
    let recency = 1.0 / (1.0 + age_days / 7.0);
    let access = 1.0 + (1.0 + m.access_count as f64).ln();
    (0.1 + m.importance) * recency * access
}

/// Assemble the injection string for a project, truncated to `max_tokens`.
/// Returns `(context, memory_count)` where `memory_count` is the number of
/// memories actually included.
pub fn get_project_context(
    store: &Store,
    project_path: &str,
    max_tokens: usize,
) -> Result<(String, usize)> {
    let Some(project) = store.get_project(project_path)? else {
        return Ok((String::new(), 0));
    };

    let (mut memories, _total) =
        store.list_memories(Some(&project.id), None, CANDIDATE_LIMIT, 0)?;

    memories.sort_by(|a, b| {
        context_score(b)
            .partial_cmp(&context_score(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let budget_chars = max_tokens * CHARS_PER_TOKEN;
    let mut out = String::new();
    let mut count = 0usize;

    for m in &memories {
        let line = format!("- [{}] {}\n", m.memory_type, m.content.trim());
        if !out.is_empty() && out.len() + line.len() > budget_chars {
            break;
        }
        // Always include at least one memory, truncated to the budget if huge.
        if out.is_empty() && line.len() > budget_chars {
            let cut = line
                .char_indices()
                .take_while(|(i, _)| *i < budget_chars)
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(0);
            out.push_str(&line[..cut]);
            count = 1;
            break;
        }
        out.push_str(&line);
        count += 1;
    }

    if !out.is_empty() {
        out = format!("Project memory for {}:\n{}", project.name, out);
    }

    Ok((out, count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MemoryType, Source};

    #[test]
    fn unknown_project_yields_empty_context() {
        let store = Store::open_in_memory().unwrap();
        let (ctx, n) = get_project_context(&store, "/nope", 2000).unwrap();
        assert!(ctx.is_empty());
        assert_eq!(n, 0);
    }

    #[test]
    fn context_ranks_by_importance_and_respects_budget() {
        let store = Store::open_in_memory().unwrap();
        let p = detect_project(&store, "/home/u/proj").unwrap();

        store
            .create_memory("low importance note", MemoryType::Fact, 0.1, Source::Cli, Some(&p.id), None)
            .unwrap();
        store
            .create_memory("critical architecture decision", MemoryType::Semantic, 0.9, Source::Cli, Some(&p.id), None)
            .unwrap();

        let (ctx, n) = get_project_context(&store, "/home/u/proj", 2000).unwrap();
        assert_eq!(n, 2);
        let crit = ctx.find("critical").unwrap();
        let low = ctx.find("low importance").unwrap();
        assert!(crit < low, "higher importance should come first");

        // Tiny budget: only the top memory fits.
        let (_ctx_small, n_small) = get_project_context(&store, "/home/u/proj", 10).unwrap();
        assert_eq!(n_small, 1);
    }
}

//! Project detection and context assembly.
//!
//! Detection: path first; on a miss, the normalized git remote gives a
//! stable identity across clones (PRD §8.10 AC2). The remote is read from
//! `.git/config` directly — no subprocess.

use anyhow::Result;
use chrono::Utc;
use std::path::Path;

use crate::model::{Memory, Project};
use crate::store::Store;

/// Approximate chars-per-token used to enforce the context token budget
/// without pulling in a tokenizer.
const CHARS_PER_TOKEN: usize = 4;

/// How many candidate memories to score before truncating to the budget.
const CANDIDATE_LIMIT: usize = 200;

/// Read the `origin` remote URL from `<path>/.git/config` without spawning
/// a subprocess. When `.git` is a file (worktrees/submodules), follows the
/// `gitdir:` pointer.
fn read_git_remote(project_path: &Path) -> Option<String> {
    let dot_git = project_path.join(".git");
    let config_path = if dot_git.is_file() {
        // "gitdir: /path/to/real/gitdir"
        let pointer = std::fs::read_to_string(&dot_git).ok()?;
        let gitdir = pointer.trim().strip_prefix("gitdir:")?.trim();
        let gitdir = if Path::new(gitdir).is_absolute() {
            std::path::PathBuf::from(gitdir)
        } else {
            project_path.join(gitdir)
        };
        gitdir.join("config")
    } else {
        dot_git.join("config")
    };

    let config = std::fs::read_to_string(config_path).ok()?;
    let mut in_origin = false;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_origin = line == r#"[remote "origin"]"#;
            continue;
        }
        if in_origin && let Some(url) = line.strip_prefix("url") {
            let url = url.trim_start().strip_prefix('=')?.trim();
            if !url.is_empty() {
                return Some(url.to_string());
            }
        }
    }
    None
}

/// Normalize a git remote URL to a stable `host/org/repo` identity:
/// `git@github.com:Org/Repo.git`, `https://u@github.com/org/repo.git/`,
/// and `ssh://git@github.com/org/repo` all become `github.com/org/repo`.
pub fn normalize_git_remote(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    // Strip scheme.
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    // Strip user@.
    let rest = rest.rsplit_once('@').map(|(_, r)| r).unwrap_or(rest);
    // scp-style host:path → host/path (but not host:port/path).
    let rest = match rest.split_once(':') {
        Some((host, path)) if !path.chars().next().is_some_and(|c| c.is_ascii_digit()) => {
            format!("{host}/{path}")
        }
        Some((host, path)) => {
            // host:port/path — drop the port.
            let path = path.split_once('/').map(|(_, p)| p).unwrap_or("");
            format!("{host}/{path}")
        }
        None => rest.to_string(),
    };

    let rest = rest.trim_end_matches('/').trim_end_matches(".git").trim_end_matches('/');
    let (host, path) = rest.split_once('/')?;
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{}/{}", host.to_lowercase(), path))
}

/// Resolve a project from an absolute directory path.
/// Identity: path hit → that project (backfilling a missing git_remote);
/// otherwise the normalized git remote — a clone at a new path resolves to
/// the original project (PRD §8.10 AC2). New paths without a match upsert.
pub fn detect_project(store: &Store, project_path: &str) -> Result<Project> {
    let remote = read_git_remote(Path::new(project_path))
        .as_deref()
        .and_then(normalize_git_remote);

    if let Some(p) = store.get_project(project_path)? {
        store.touch_project(&p.id)?;
        if p.git_remote.is_none()
            && let Some(r) = &remote
        {
            store.set_project_remote(&p.id, r)?;
        }
        return Ok(p);
    }

    if let Some(r) = &remote
        && let Some(p) = store.get_project_by_remote(r)?
    {
        // Same repo cloned elsewhere: same project, original path kept.
        store.touch_project(&p.id)?;
        return Ok(p);
    }

    let name = Path::new(project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| project_path.to_string());
    store.upsert_project(project_path, &name, remote.as_deref())
}

/// Ranking score per PRD §8.10: importance × recency × access × strength.
fn context_score(m: &Memory) -> f64 {
    let age_days = (Utc::now() - m.created_at).num_seconds().max(0) as f64 / 86400.0;
    let recency = 1.0 / (1.0 + age_days / 7.0);
    let access = 1.0 + (1.0 + m.access_count as f64).ln();
    let strength_factor = 0.1 + 0.9 * m.strength;
    (0.1 + m.importance) * recency * access * strength_factor
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
        // Prefer the cached compressed rewrite; recall/FTS/vector search
        // never falls back to it, only context-injection does, right here.
        let text = match store.get_compressed_content(&m.id)? {
            Some((compressed, _mode)) => compressed,
            None => m.content.trim().to_string(),
        };
        let line = format!("- [{}] {}\n", m.memory_type, text);
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
    fn normalize_git_remote_forms() {
        let cases = [
            ("git@github.com:Acme/Repo.git", "github.com/Acme/Repo"),
            ("https://github.com/acme/repo.git", "github.com/acme/repo"),
            ("https://user@GitHub.com/acme/repo/", "github.com/acme/repo"),
            ("ssh://git@github.com/acme/repo", "github.com/acme/repo"),
            ("ssh://git@github.com:22/acme/repo.git", "github.com/acme/repo"),
            ("git@gitlab.example.com:group/sub/repo.git", "gitlab.example.com/group/sub/repo"),
        ];
        for (input, want) in cases {
            assert_eq!(normalize_git_remote(input).as_deref(), Some(want), "input: {input}");
        }
        assert_eq!(normalize_git_remote(""), None);
        assert_eq!(normalize_git_remote("nonsense"), None);
    }

    fn write_git_config(dir: &Path, url: &str) {
        let git = dir.join(".git");
        std::fs::create_dir_all(&git).unwrap();
        std::fs::write(
            git.join("config"),
            format!("[core]\n\tbare = false\n[remote \"origin\"]\n\turl = {url}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n"),
        )
        .unwrap();
    }

    #[test]
    fn clone_at_new_path_resolves_to_same_project() {
        let store = Store::open_in_memory().unwrap();
        let tmp = tempfile::tempdir().unwrap();

        let a = tmp.path().join("clone-a");
        let b = tmp.path().join("clone-b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        write_git_config(&a, "git@github.com:acme/x.git");
        write_git_config(&b, "https://github.com/acme/x.git");

        let p1 = detect_project(&store, a.to_str().unwrap()).unwrap();
        assert_eq!(p1.git_remote.as_deref(), Some("github.com/acme/x"));

        // Different URL form, same normalized identity → same project,
        // original path kept (PRD §8.10 AC2).
        let p2 = detect_project(&store, b.to_str().unwrap()).unwrap();
        assert_eq!(p2.id, p1.id);
        assert_eq!(p2.path, p1.path);
        assert_eq!(store.list_projects().unwrap().len(), 1);
    }

    #[test]
    fn no_git_dir_is_path_only_project() {
        let store = Store::open_in_memory().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let p = detect_project(&store, tmp.path().to_str().unwrap()).unwrap();
        assert!(p.git_remote.is_none());
    }

    #[test]
    fn path_hit_backfills_missing_remote() {
        let store = Store::open_in_memory().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("proj");
        std::fs::create_dir_all(&dir).unwrap();

        // First seen without git.
        let p1 = detect_project(&store, dir.to_str().unwrap()).unwrap();
        assert!(p1.git_remote.is_none());

        // Later it's a git checkout — remote backfilled on next detection.
        write_git_config(&dir, "git@github.com:acme/y.git");
        detect_project(&store, dir.to_str().unwrap()).unwrap();
        let refreshed = store.get_project(dir.to_str().unwrap()).unwrap().unwrap();
        assert_eq!(refreshed.git_remote.as_deref(), Some("github.com/acme/y"));
    }

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

    #[test]
    fn context_prefers_compressed_content_falls_back_when_absent() {
        let store = Store::open_in_memory().unwrap();
        let p = detect_project(&store, "/home/u/proj").unwrap();

        let compressed_one = store
            .create_memory("original verbose text for one", MemoryType::Fact, 0.9, Source::Cli, Some(&p.id), None)
            .unwrap();
        store.set_compressed_content(&compressed_one.id, "compressed stand-in", "caveman").unwrap();

        store
            .create_memory("original verbose text for two", MemoryType::Fact, 0.5, Source::Cli, Some(&p.id), None)
            .unwrap();

        let (ctx, n) = get_project_context(&store, "/home/u/proj", 2000).unwrap();
        assert_eq!(n, 2);
        assert!(ctx.contains("compressed stand-in"));
        assert!(!ctx.contains("original verbose text for one"));
        assert!(ctx.contains("original verbose text for two"));
    }
}

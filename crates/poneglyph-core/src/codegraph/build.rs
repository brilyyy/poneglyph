//! Walks a project directory, parses each matching source file, and
//! persists the result. Two passes per run: nodes first (so every symbol
//! exists in the DB), then call/test edge resolution (so forward
//! references — a call to a function defined in a file parsed later in
//! the same run — still resolve).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::CodeGraphConfig;
use crate::model::{CgEdge, CgEdgeKind, CgFile, CgNodeKind};
use crate::privacy::build_exclude_matcher;
use crate::store::Store;

use super::parse::{self, ParsedFile};

#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct BuildReport {
    pub files_parsed: usize,
    pub files_unchanged: usize,
    pub files_removed: usize,
    pub nodes: usize,
    pub edges: usize,
}

/// `force = true` (full `graph init`) reparses every matching file.
/// `force = false` (`graph update`) skips files whose content hash hasn't
/// changed since the last build.
pub fn build(store: &Store, root: &Path, config: &CodeGraphConfig, force: bool) -> Result<BuildReport> {
    let exclude = build_exclude_matcher(&config.exclude_patterns);
    let candidates = collect_candidates(root, &exclude);

    let mut current_paths = std::collections::HashSet::new();
    let mut report = BuildReport::default();
    // (file_path, file_language, parsed) for files we re-parsed this run —
    // edge resolution happens after every file's nodes are persisted.
    let mut pending: Vec<(String, ParsedFile)> = Vec::new();

    for path in &candidates {
        let rel = relative_slash_path(root, path);
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else { continue };
        let Some(language) = parse::language_for_extension(ext) else { continue };
        if !config.languages.is_empty() && !config.languages.iter().any(|l| l == language) {
            continue;
        }
        current_paths.insert(rel.clone());

        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => continue, // binary/non-UTF8 file masquerading under a known extension
        };
        let hash = content_hash(&source);

        if !force && store.cg_file_hash(&rel)?.as_deref() == Some(hash.as_str()) {
            report.files_unchanged += 1;
            continue;
        }

        let parsed = parse::parse_file(&rel, language, &source).with_context(|| format!("failed to parse {rel}"))?;
        store.cg_clear_file(&rel)?;
        store.cg_upsert_file(&CgFile { path: rel.clone(), language: language.to_string(), content_hash: hash })?;
        for node in &parsed.nodes {
            store.cg_insert_node(node)?;
        }
        report.files_parsed += 1;
        report.nodes += parsed.nodes.len();
        pending.push((rel, parsed));
    }

    // Files that existed in a previous build but were deleted or excluded this run.
    for file in store.cg_all_files()? {
        if !current_paths.contains(&file.path) {
            store.cg_clear_file(&file.path)?;
            report.files_removed += 1;
        }
    }

    for (file_path, parsed) in &pending {
        report.edges += resolve_edges(store, file_path, parsed)?;
    }

    Ok(report)
}

/// Name-only resolution: no type/scope information, so an overloaded or
/// duplicate name picks whichever match comes back first. Acceptable for a
/// "what's nearby" code graph; not a substitute for a real type checker.
fn resolve_edges(store: &Store, file_path: &str, parsed: &ParsedFile) -> Result<usize> {
    let mut count = 0;

    for (caller_id, callee_name) in &parsed.calls {
        let Some(caller_id) = caller_id else { continue };
        let candidates = store.cg_nodes_by_name(callee_name, &[CgNodeKind::Function, CgNodeKind::Method])?;
        let Some(target) = candidates.into_iter().next() else { continue };
        if target.id == *caller_id {
            continue; // skip self-recursive calls — not useful for blast-radius fan-out
        }
        if store.cg_insert_edge(&CgEdge { src_id: caller_id.clone(), dst_id: target.id, kind: CgEdgeKind::Calls })? {
            count += 1;
        }
    }

    for (test_id, target_guess) in &parsed.tests {
        let Some(name) = target_guess else { continue };
        let candidates = store.cg_nodes_by_name(name, &[CgNodeKind::Function, CgNodeKind::Method])?;
        let target = candidates.iter().find(|n| n.file_path == file_path).or_else(|| candidates.first());
        let Some(target) = target else { continue };
        if store.cg_insert_edge(&CgEdge { src_id: test_id.clone(), dst_id: target.id.clone(), kind: CgEdgeKind::Tests })? {
            count += 1;
        }
    }

    Ok(count)
}

fn content_hash(source: &str) -> String {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn relative_slash_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

/// Walks `root` honoring a project-local `.poneglyphignore` (gitignore
/// syntax — negation, directory anchoring, etc. via the `ignore` crate)
/// merged with `config.exclude_patterns` (globset, checked separately):
/// either source can exclude a path, neither overrides the other into
/// inclusion. Deliberately does not honor `.gitignore`/`.git/info/exclude`/
/// the user's global gitignore — this is `.poneglyphignore`-only so existing
/// configs don't change behavior just because a repo has a `.gitignore`.
/// Permission-denied subdirs etc. are skipped rather than aborting the build.
fn collect_candidates(root: &Path, exclude: &globset::GlobSet) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .parents(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .add_custom_ignore_filename(".poneglyphignore")
        .build();

    for entry in walker {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let rel = relative_slash_path(root, path);
        if exclude.is_match(&rel) {
            continue;
        }
        out.push(path.to_path_buf());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use tempfile::tempdir;

    fn cfg() -> CodeGraphConfig {
        CodeGraphConfig::default()
    }

    #[test]
    fn build_parses_matching_files_and_skips_excluded() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("lib.rs"), "fn foo() -> i32 { 1 }\n").unwrap();
        std::fs::create_dir_all(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target/generated.rs"), "fn ignored() {}\n").unwrap();

        let store = Store::open_in_memory().unwrap();
        let report = build(&store, dir.path(), &cfg(), true).unwrap();

        assert_eq!(report.files_parsed, 1);
        assert_eq!(report.nodes, 1);
        let nodes = store.cg_nodes_in_file("lib.rs").unwrap();
        assert_eq!(nodes[0].name, "foo");
    }

    #[test]
    fn build_resolves_forward_reference_calls_across_files() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn a() { b(); }\n").unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn b() -> i32 { 1 }\n").unwrap();

        let store = Store::open_in_memory().unwrap();
        let report = build(&store, dir.path(), &cfg(), true).unwrap();

        assert_eq!(report.edges, 1, "call to a function defined in a later file must still resolve");
    }

    #[test]
    fn update_skips_unchanged_files_and_reparses_changed_ones() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.rs");
        std::fs::write(&path, "fn a() {}\n").unwrap();

        let store = Store::open_in_memory().unwrap();
        build(&store, dir.path(), &cfg(), true).unwrap();

        let report1 = build(&store, dir.path(), &cfg(), false).unwrap();
        assert_eq!(report1.files_unchanged, 1);
        assert_eq!(report1.files_parsed, 0);

        std::fs::write(&path, "fn a() {}\nfn b() {}\n").unwrap();
        let report2 = build(&store, dir.path(), &cfg(), false).unwrap();
        assert_eq!(report2.files_parsed, 1);
        assert_eq!(store.cg_nodes_in_file("a.rs").unwrap().len(), 2);
    }

    #[test]
    fn build_removes_nodes_for_deleted_files() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.rs");
        std::fs::write(&path, "fn a() {}\n").unwrap();

        let store = Store::open_in_memory().unwrap();
        build(&store, dir.path(), &cfg(), true).unwrap();
        assert_eq!(store.cg_all_files().unwrap().len(), 1);

        std::fs::remove_file(&path).unwrap();
        let report = build(&store, dir.path(), &cfg(), false).unwrap();
        assert_eq!(report.files_removed, 1);
        assert!(store.cg_all_files().unwrap().is_empty());
    }

    #[test]
    fn build_resolves_test_to_target_by_naming_convention() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.py"), "def add(a, b):\n    return a + b\n\ndef test_add():\n    assert add(1, 2) == 3\n").unwrap();

        let store = Store::open_in_memory().unwrap();
        let report = build(&store, dir.path(), &cfg(), true).unwrap();
        // 1 call edge (test_add -> add) + 1 test edge (test_add -> add)
        assert_eq!(report.edges, 2);
    }

    #[test]
    fn build_respects_poneglyphignore_file() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("kept.rs"), "fn kept() {}\n").unwrap();
        std::fs::create_dir_all(dir.path().join("ignored_dir")).unwrap();
        std::fs::write(dir.path().join("ignored_dir/skip.rs"), "fn skip() {}\n").unwrap();
        std::fs::write(dir.path().join(".poneglyphignore"), "ignored_dir/\n").unwrap();

        let store = Store::open_in_memory().unwrap();
        let report = build(&store, dir.path(), &cfg(), true).unwrap();

        assert_eq!(report.files_parsed, 1);
        assert_eq!(store.cg_nodes_in_file("kept.rs").unwrap().len(), 1);
        assert!(store.cg_nodes_in_file("ignored_dir/skip.rs").unwrap().is_empty());
    }

    #[test]
    fn build_poneglyphignore_negation_pattern() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
        // a.rs would normally be excluded by "*.rs", but "!a.rs" un-excludes it —
        // the concrete payoff of gitignore-correct negation handling.
        std::fs::write(dir.path().join(".poneglyphignore"), "*.rs\n!a.rs\n").unwrap();

        let store = Store::open_in_memory().unwrap();
        let report = build(&store, dir.path(), &cfg(), true).unwrap();
        assert_eq!(report.files_parsed, 1);
        assert_eq!(store.cg_nodes_in_file("a.rs").unwrap().len(), 1);
    }

    #[test]
    fn build_merges_poneglyphignore_with_config_exclude_patterns() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn b() {}\n").unwrap();
        std::fs::write(dir.path().join(".poneglyphignore"), "a.rs\n").unwrap();

        let mut config = cfg();
        config.exclude_patterns = vec!["b.rs".to_string()];

        let store = Store::open_in_memory().unwrap();
        let report = build(&store, dir.path(), &config, true).unwrap();

        // Both excluded — neither source overrides the other into inclusion.
        assert_eq!(report.files_parsed, 0);
        assert!(store.cg_nodes_in_file("a.rs").unwrap().is_empty());
        assert!(store.cg_nodes_in_file("b.rs").unwrap().is_empty());
    }

    #[test]
    fn build_respects_language_allowlist() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(dir.path().join("a.py"), "def a():\n    pass\n").unwrap();

        let mut config = cfg();
        config.languages = vec!["python".to_string()];

        let store = Store::open_in_memory().unwrap();
        let report = build(&store, dir.path(), &config, true).unwrap();
        assert_eq!(report.files_parsed, 1);
        assert!(store.cg_nodes_in_file("a.py").unwrap().len() == 1);
        assert!(store.cg_nodes_in_file("a.rs").unwrap().is_empty());
    }
}

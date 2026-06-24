//! `~/.config/poneglyph/graph_projects.toml` — the list of projects that
//! have run `graph init`/`graph update`, so the `poneglyph mcp` daemon's
//! background task (see `spawn_graph_auto_update` in main.rs) knows which
//! directories to incrementally re-parse without the user running it by
//! hand. Strictly for that; not a general project registry (memories use
//! their own `projects` table in the SQLite store).

use anyhow::{Context, Result};
use poneglyph_core::config::Config;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct GraphProjects {
    #[serde(default, rename = "project")]
    pub project: Vec<GraphProjectEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphProjectEntry {
    pub dir: String,
    pub id: String,
}

fn registry_path() -> std::path::PathBuf {
    Config::config_dir().join("graph_projects.toml")
}

pub fn load() -> Result<GraphProjects> {
    let path = registry_path();
    if !path.exists() {
        return Ok(GraphProjects::default());
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

fn save(projects: &GraphProjects) -> Result<()> {
    let path = registry_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let toml = toml::to_string_pretty(projects).context("failed to serialize graph_projects.toml")?;
    std::fs::write(&path, toml).with_context(|| format!("failed to write {}", path.display()))
}

/// Track `dir` (already canonicalized by the caller) under `id`. Upserts by
/// `dir` — re-registering an already-tracked project just updates its id.
/// ponytail: whole-file read+rewrite per call — fine at "a handful of
/// projects you've run `graph init` in"; revisit if that grows large.
pub fn register(dir: &Path, id: &str) -> Result<()> {
    let dir_str = dir.to_string_lossy().to_string();
    let mut projects = load()?;
    match projects.project.iter_mut().find(|p| p.dir == dir_str) {
        Some(existing) => existing.id = id.to_string(),
        None => projects.project.push(GraphProjectEntry {
            dir: dir_str,
            id: id.to_string(),
        }),
    }
    save(&projects)
}

/// Remove the entry for `dir` if present. No-op if not found.
pub fn unregister(dir: &str) -> Result<()> {
    let mut projects = load()?;
    let before = projects.project.len();
    projects.project.retain(|p| p.dir != dir);
    if projects.project.len() != before {
        save(&projects)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_dedups_by_dir() {
        let mut projects = GraphProjects::default();
        projects.project.push(GraphProjectEntry {
            dir: "/a".into(),
            id: "1".into(),
        });
        // Same logic register() applies, exercised directly so this test
        // doesn't depend on a writable ~/.config/poneglyph.
        match projects.project.iter_mut().find(|p| p.dir == "/a") {
            Some(existing) => existing.id = "2".into(),
            None => projects.project.push(GraphProjectEntry {
                dir: "/a".into(),
                id: "2".into(),
            }),
        }
        assert_eq!(projects.project.len(), 1);
        assert_eq!(projects.project[0].id, "2");
    }

    #[test]
    fn toml_round_trips() {
        let mut projects = GraphProjects::default();
        projects.project.push(GraphProjectEntry {
            dir: "/some/project".into(),
            id: "abc-123".into(),
        });
        let rendered = toml::to_string_pretty(&projects).unwrap();
        let parsed: GraphProjects = toml::from_str(&rendered).unwrap();
        assert_eq!(parsed.project, projects.project);
    }
}

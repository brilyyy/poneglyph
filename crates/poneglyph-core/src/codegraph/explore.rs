//! Single-call "show me everything about this symbol": composes
//! `blast_radius` plus direct caller/callee/supertype/subtype edges and a
//! source-snippet read, so one MCP/CLI call covers what otherwise takes a
//! `codegraph_query` + `codegraph_blast_radius` + manual `Read`. Does not
//! duplicate the BFS/lookup logic those already implement.

use std::path::Path;

use anyhow::Result;

use crate::model::{CgEdgeKind, CgNode, CgNodeKind};
use crate::store::Store;

use super::blast_radius::{self, BlastRadiusReport};

#[derive(Debug, Default, serde::Serialize)]
pub struct ExploreReport {
    /// The file's symbols, or the matching symbol(s) by name — same
    /// resolution `blast_radius` uses.
    pub root: Vec<CgNode>,
    /// Source text for each root symbol, read from disk at its line range.
    pub snippets: Vec<Snippet>,
    pub callers: Vec<CgNode>,
    pub callees: Vec<CgNode>,
    /// What a root `Type` symbol extends/implements.
    pub supertypes: Vec<CgNode>,
    /// What extends/implements a root `Type` symbol.
    pub subtypes: Vec<CgNode>,
    pub tests: Vec<CgNode>,
    /// Full transitive caller/importer/test trace, depth-bounded — `root`/
    /// `tests` above are repeated here too (nested with depths); the
    /// top-level fields are the "quick glance", this is the full picture.
    pub blast_radius: BlastRadiusReport,
}

#[derive(Debug, serde::Serialize)]
pub struct Snippet {
    pub node_id: String,
    pub file_path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub source: String,
}

pub fn explore(store: &Store, project_id: &str, project_root: &Path, target: &str, max_depth: usize) -> Result<ExploreReport> {
    let blast = blast_radius::blast_radius(store, project_id, target, max_depth)?;
    if blast.root.is_empty() {
        return Ok(ExploreReport::default());
    }

    let mut snippets = Vec::new();
    let mut callers = Vec::new();
    let mut callees = Vec::new();
    let mut supertypes = Vec::new();
    let mut subtypes = Vec::new();

    for node in &blast.root {
        if let Ok(source) = read_snippet(project_root, &node.file_path, node.start_line, node.end_line) {
            snippets.push(Snippet {
                node_id: node.id.clone(),
                file_path: node.file_path.clone(),
                start_line: node.start_line,
                end_line: node.end_line,
                source,
            });
        }
        callers.extend(store.cg_edges_into(project_id, &node.id, CgEdgeKind::Calls)?);
        callees.extend(store.cg_edges_out_of(project_id, &node.id, CgEdgeKind::Calls)?);
        if node.kind == CgNodeKind::Type {
            subtypes.extend(store.cg_edges_into(project_id, &node.id, CgEdgeKind::Extends)?);
            supertypes.extend(store.cg_edges_out_of(project_id, &node.id, CgEdgeKind::Extends)?);
        }
    }

    Ok(ExploreReport {
        root: blast.root.clone(),
        snippets,
        callers,
        callees,
        supertypes,
        subtypes,
        tests: blast.tests.clone(),
        blast_radius: blast,
    })
}

fn read_snippet(root: &Path, file_path: &str, start_line: usize, end_line: usize) -> Result<String> {
    let content = std::fs::read_to_string(root.join(file_path))?;
    let lines: Vec<&str> = content.lines().collect();
    let start = start_line.saturating_sub(1).min(lines.len());
    let end = end_line.min(lines.len());
    Ok(lines[start..end].join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CodeGraphConfig;
    use tempfile::tempdir;

    fn pid(store: &Store, dir: &Path) -> String {
        crate::project::detect_project(store, &dir.to_string_lossy()).unwrap().id
    }

    #[test]
    fn explores_a_function_with_snippet_and_callers() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.py"), "def add(a, b):\n    return a + b\n\ndef use_add():\n    return add(1, 2)\n").unwrap();
        let store = Store::open_in_memory().unwrap();
        super::super::build::build(&store, dir.path(), &CodeGraphConfig::default(), true).unwrap();
        let project_id = pid(&store, dir.path());

        let report = explore(&store, &project_id, dir.path(), "add", 5).unwrap();
        assert_eq!(report.root.len(), 1);
        assert_eq!(report.root[0].name, "add");
        assert_eq!(report.snippets.len(), 1);
        assert_eq!(report.snippets[0].source, "def add(a, b):\n    return a + b");
        assert!(report.callers.iter().any(|n| n.name == "use_add"));
    }

    #[test]
    fn explores_a_type_with_supertypes_and_subtypes() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.py"), "class Animal:\n    pass\n\nclass Dog(Animal):\n    pass\n").unwrap();
        let store = Store::open_in_memory().unwrap();
        super::super::build::build(&store, dir.path(), &CodeGraphConfig::default(), true).unwrap();
        let project_id = pid(&store, dir.path());

        let report = explore(&store, &project_id, dir.path(), "Animal", 5).unwrap();
        assert!(report.subtypes.iter().any(|n| n.name == "Dog"));

        let report = explore(&store, &project_id, dir.path(), "Dog", 5).unwrap();
        assert!(report.supertypes.iter().any(|n| n.name == "Animal"));
    }

    #[test]
    fn unknown_target_returns_empty_report() {
        let store = Store::open_in_memory().unwrap();
        let report = explore(&store, "no-such-project", Path::new("/nonexistent"), "nonexistent", 5).unwrap();
        assert!(report.root.is_empty());
        assert!(report.snippets.is_empty());
    }
}

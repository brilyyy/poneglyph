//! Recursive caller/importer/test trace from a file or symbol, bounded by
//! `[graph].blast_radius_depth`. Answers "what breaks if I change this".

use std::collections::HashMap;

use anyhow::Result;

use crate::model::{CgEdgeKind, CgNode, CgNodeKind};
use crate::store::Store;

#[derive(Debug, Default, serde::Serialize)]
pub struct BlastRadiusReport {
    /// The file's symbols, or the matching symbol(s) by name.
    pub root: Vec<CgNode>,
    /// Transitive callers/importers, nearest first, deduped.
    pub dependents: Vec<DependentNode>,
    /// Tests covering anything in `root` or `dependents`.
    pub tests: Vec<CgNode>,
}

#[derive(Debug, serde::Serialize)]
pub struct DependentNode {
    pub node: CgNode,
    pub depth: usize,
}

pub fn blast_radius(store: &Store, project_id: &str, target: &str, max_depth: usize) -> Result<BlastRadiusReport> {
    let by_file = store.cg_nodes_in_file(project_id, target)?;
    let root = if !by_file.is_empty() {
        by_file
    } else {
        store.cg_nodes_by_name(project_id, target, &[CgNodeKind::Function, CgNodeKind::Method, CgNodeKind::Type])?
    };
    if root.is_empty() {
        return Ok(BlastRadiusReport::default());
    }

    let mut visited: HashMap<String, usize> = root.iter().map(|n| (n.id.clone(), 0)).collect();
    let mut dependents = Vec::new();
    let mut frontier: Vec<CgNode> = root.clone();
    let mut depth = 0;

    while depth < max_depth && !frontier.is_empty() {
        depth += 1;
        let mut next = Vec::new();
        for node in &frontier {
            let mut callers = store.cg_edges_into(project_id, &node.id, CgEdgeKind::Calls)?;
            callers.extend(store.cg_edges_into(project_id, &node.id, CgEdgeKind::Imports)?);
            callers.extend(store.cg_edges_into(project_id, &node.id, CgEdgeKind::Extends)?);
            for caller in callers {
                if visited.contains_key(&caller.id) {
                    continue;
                }
                visited.insert(caller.id.clone(), depth);
                dependents.push(DependentNode { node: caller.clone(), depth });
                next.push(caller);
            }
        }
        frontier = next;
    }

    let mut tests = Vec::new();
    let mut seen_tests = std::collections::HashSet::new();
    for id in visited.keys() {
        for test in store.cg_edges_into(project_id, id, CgEdgeKind::Tests)? {
            if seen_tests.insert(test.id.clone()) {
                tests.push(test);
            }
        }
    }

    Ok(BlastRadiusReport { root, dependents, tests })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CodeGraphConfig;
    use crate::store::Store;
    use tempfile::tempdir;

    fn pid(store: &Store, dir: &std::path::Path) -> String {
        crate::project::detect_project(store, &dir.to_string_lossy()).unwrap().id
    }

    #[test]
    fn traces_transitive_callers_bounded_by_depth() {
        let dir = tempdir().unwrap();
        // d -> c -> b -> a (each calls the previous)
        std::fs::write(dir.path().join("chain.rs"), "fn a() {}\nfn b() { a(); }\nfn c() { b(); }\nfn d() { c(); }\n").unwrap();
        let store = Store::open_in_memory().unwrap();
        super::super::build::build(&store, dir.path(), &CodeGraphConfig::default(), true).unwrap();
        let project_id = pid(&store, dir.path());

        let report = blast_radius(&store, &project_id, "a", 5).unwrap();
        let names: Vec<&str> = report.dependents.iter().map(|d| d.node.name.as_str()).collect();
        assert_eq!(names, vec!["b", "c", "d"]);
        assert_eq!(report.dependents.iter().find(|d| d.node.name == "b").unwrap().depth, 1);
        assert_eq!(report.dependents.iter().find(|d| d.node.name == "d").unwrap().depth, 3);

        let shallow = blast_radius(&store, &project_id, "a", 1).unwrap();
        assert_eq!(shallow.dependents.len(), 1);
        assert_eq!(shallow.dependents[0].node.name, "b");
    }

    #[test]
    fn extends_edges_surface_implementors_in_dependents() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.py"), "class Animal:\n    pass\n\nclass Dog(Animal):\n    pass\n").unwrap();
        let store = Store::open_in_memory().unwrap();
        super::super::build::build(&store, dir.path(), &CodeGraphConfig::default(), true).unwrap();
        let project_id = pid(&store, dir.path());

        let report = blast_radius(&store, &project_id, "Animal", 5).unwrap();
        assert!(report.dependents.iter().any(|d| d.node.name == "Dog"), "changing the base class should flag its subclass");
    }

    #[test]
    fn includes_covering_tests() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.py"), "def add(a, b):\n    return a + b\n\ndef test_add():\n    assert add(1, 2) == 3\n").unwrap();
        let store = Store::open_in_memory().unwrap();
        super::super::build::build(&store, dir.path(), &CodeGraphConfig::default(), true).unwrap();
        let project_id = pid(&store, dir.path());

        let report = blast_radius(&store, &project_id, "add", 5).unwrap();
        assert_eq!(report.tests.len(), 1);
        assert_eq!(report.tests[0].name, "test_add");
    }

    #[test]
    fn unknown_target_returns_empty() {
        let store = Store::open_in_memory().unwrap();
        let report = blast_radius(&store, "no-such-project", "nonexistent", 5).unwrap();
        assert!(report.root.is_empty());
        assert!(report.dependents.is_empty());
    }

    #[test]
    fn file_target_uses_all_symbols_in_file_as_roots() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("lib.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        std::fs::write(dir.path().join("caller.rs"), "fn uses_a() { a(); }\n").unwrap();
        let store = Store::open_in_memory().unwrap();
        super::super::build::build(&store, dir.path(), &CodeGraphConfig::default(), true).unwrap();
        let project_id = pid(&store, dir.path());

        let report = blast_radius(&store, &project_id, "lib.rs", 5).unwrap();
        assert_eq!(report.root.len(), 2);
        assert!(report.dependents.iter().any(|d| d.node.name == "uses_a"));
    }
}

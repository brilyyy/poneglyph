//! Structured queries over the code graph: callers_of / callees_of /
//! imports_of / tests_for (all "who depends on this", i.e. reverse edges,
//! except callees_of which is inherently forward) plus a keyword fallback.

use std::collections::{HashMap, VecDeque};

use anyhow::Result;

use crate::model::{CgEdgeKind, CgNode, CgNodeKind};
use crate::store::Store;

/// A parsed `poneglyph graph query` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphQuery {
    CallersOf(String),
    CalleesOf(String),
    ImportsOf(String),
    TestsFor(String),
    /// `path:<from>..<to>` — shortest call/import chain between two symbols.
    Path(String, String),
    Keyword(String),
}

/// Parses `callers_of:<name>`, `callees_of:<name>`, `imports_of:<name>`,
/// `tests_for:<name>`, `path:<from>..<to>`; anything else (including a bare
/// name) is a keyword search.
pub fn parse_query(input: &str) -> GraphQuery {
    let trimmed = input.trim();
    for (prefix, ctor) in [
        ("callers_of:", GraphQuery::CallersOf as fn(String) -> GraphQuery),
        ("callees_of:", GraphQuery::CalleesOf as fn(String) -> GraphQuery),
        ("imports_of:", GraphQuery::ImportsOf as fn(String) -> GraphQuery),
        ("tests_for:", GraphQuery::TestsFor as fn(String) -> GraphQuery),
    ] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return ctor(rest.trim().to_string());
        }
    }
    if let Some(rest) = trimmed.strip_prefix("path:") {
        if let Some((from, to)) = rest.split_once("..") {
            return GraphQuery::Path(from.trim().to_string(), to.trim().to_string());
        }
    }
    GraphQuery::Keyword(trimmed.to_string())
}

fn first_match_by_name(store: &Store, name: &str) -> Result<Option<CgNode>> {
    Ok(store.cg_nodes_by_name(name, &[CgNodeKind::Function, CgNodeKind::Method, CgNodeKind::Type])?.into_iter().next())
}

pub fn run(store: &Store, query: &GraphQuery) -> Result<Vec<CgNode>> {
    match query {
        GraphQuery::CallersOf(name) => {
            let Some(target) = first_match_by_name(store, name)? else { return Ok(Vec::new()) };
            store.cg_edges_into(&target.id, CgEdgeKind::Calls)
        }
        GraphQuery::CalleesOf(name) => {
            let Some(target) = first_match_by_name(store, name)? else { return Ok(Vec::new()) };
            store.cg_edges_out_of(&target.id, CgEdgeKind::Calls)
        }
        GraphQuery::ImportsOf(name) => {
            let candidates = store.cg_nodes_by_name(name, &[CgNodeKind::Import])?;
            let mut out = Vec::new();
            for c in candidates {
                out.extend(store.cg_edges_into(&c.id, CgEdgeKind::Imports)?);
            }
            if out.is_empty() {
                // Fall back to substring match on import text (the raw `use`/`import` statement).
                out = store
                    .cg_search_by_name(name, 50)?
                    .into_iter()
                    .filter(|n| n.kind == CgNodeKind::Import)
                    .collect();
            }
            Ok(out)
        }
        GraphQuery::TestsFor(name) => {
            let Some(target) = first_match_by_name(store, name)? else { return Ok(Vec::new()) };
            store.cg_edges_into(&target.id, CgEdgeKind::Tests)
        }
        GraphQuery::Path(from, to) => shortest_path(store, from, to),
        GraphQuery::Keyword(kw) => store.cg_search_by_name(kw, 50),
    }
}

/// BFS over forward Calls+Imports edges — shortest hop count, not weighted.
/// Returns the node chain from `from` to `to` inclusive, or empty if either
/// symbol is unknown or no path exists within the graph.
fn shortest_path(store: &Store, from: &str, to: &str) -> Result<Vec<CgNode>> {
    let Some(start) = first_match_by_name(store, from)? else { return Ok(Vec::new()) };
    let Some(end) = first_match_by_name(store, to)? else { return Ok(Vec::new()) };
    if start.id == end.id {
        return Ok(vec![start]);
    }

    let mut parent: HashMap<String, String> = HashMap::new();
    let mut visited: HashMap<String, CgNode> = HashMap::new();
    let mut queue = VecDeque::new();
    visited.insert(start.id.clone(), start.clone());
    queue.push_back(start.id.clone());

    while let Some(current_id) = queue.pop_front() {
        let mut neighbors = store.cg_edges_out_of(&current_id, CgEdgeKind::Calls)?;
        neighbors.extend(store.cg_edges_out_of(&current_id, CgEdgeKind::Imports)?);

        for next in neighbors {
            if visited.contains_key(&next.id) {
                continue;
            }
            parent.insert(next.id.clone(), current_id.clone());
            if next.id == end.id {
                return Ok(reconstruct_path(&parent, &visited, &start, &next));
            }
            visited.insert(next.id.clone(), next.clone());
            queue.push_back(next.id);
        }
    }

    Ok(Vec::new())
}

fn reconstruct_path(parent: &HashMap<String, String>, visited: &HashMap<String, CgNode>, start: &CgNode, end: &CgNode) -> Vec<CgNode> {
    let mut chain = vec![end.clone()];
    let mut current = end.id.clone();
    while let Some(prev_id) = parent.get(&current) {
        if *prev_id == start.id {
            chain.push(start.clone());
            break;
        }
        chain.push(visited[prev_id].clone());
        current = prev_id.clone();
    }
    chain.reverse();
    chain
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CodeGraphConfig;
    use crate::store::Store;
    use tempfile::tempdir;

    fn build_fixture() -> (tempfile::TempDir, Store) {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.py"), "def add(a, b):\n    return a + b\n\ndef test_add():\n    assert add(1, 2) == 3\n").unwrap();
        std::fs::write(dir.path().join("b.py"), "from a import add\n\ndef use_add():\n    return add(1, 2)\n").unwrap();
        let store = Store::open_in_memory().unwrap();
        super::super::build::build(&store, dir.path(), &CodeGraphConfig::default(), true).unwrap();
        (dir, store)
    }

    #[test]
    fn parse_query_recognizes_all_prefixes() {
        assert_eq!(parse_query("callers_of:foo"), GraphQuery::CallersOf("foo".into()));
        assert_eq!(parse_query("callees_of:foo"), GraphQuery::CalleesOf("foo".into()));
        assert_eq!(parse_query("imports_of:foo"), GraphQuery::ImportsOf("foo".into()));
        assert_eq!(parse_query("tests_for:foo"), GraphQuery::TestsFor("foo".into()));
        assert_eq!(parse_query("path:foo..bar"), GraphQuery::Path("foo".into(), "bar".into()));
        assert_eq!(parse_query("foo"), GraphQuery::Keyword("foo".into()));
    }

    #[test]
    fn path_finds_shortest_call_chain() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("chain.py"), "def a():\n    b()\n\ndef b():\n    c()\n\ndef c():\n    pass\n").unwrap();
        let store = Store::open_in_memory().unwrap();
        super::super::build::build(&store, dir.path(), &CodeGraphConfig::default(), true).unwrap();

        let path = run(&store, &GraphQuery::Path("a".into(), "c".into())).unwrap();
        let names: Vec<&str> = path.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn path_returns_empty_when_unreachable() {
        let (_dir, store) = build_fixture();
        let path = run(&store, &GraphQuery::Path("add".into(), "use_add".into())).unwrap();
        assert!(path.is_empty(), "add doesn't call use_add, no forward path exists");
    }

    #[test]
    fn callers_of_finds_both_callers() {
        let (_dir, store) = build_fixture();
        let callers = run(&store, &GraphQuery::CallersOf("add".into())).unwrap();
        let names: Vec<&str> = callers.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"test_add"));
        assert!(names.contains(&"use_add"));
    }

    #[test]
    fn callees_of_is_forward() {
        let (_dir, store) = build_fixture();
        let callees = run(&store, &GraphQuery::CalleesOf("use_add".into())).unwrap();
        assert_eq!(callees.len(), 1);
        assert_eq!(callees[0].name, "add");
    }

    #[test]
    fn tests_for_returns_test_node() {
        let (_dir, store) = build_fixture();
        let tests = run(&store, &GraphQuery::TestsFor("add".into())).unwrap();
        assert_eq!(tests.len(), 1);
        assert_eq!(tests[0].name, "test_add");
    }

    #[test]
    fn keyword_search_matches_substring() {
        let (_dir, store) = build_fixture();
        let results = run(&store, &GraphQuery::Keyword("add".into())).unwrap();
        assert!(results.len() >= 2);
    }

    #[test]
    fn unknown_symbol_returns_empty_not_error() {
        let (_dir, store) = build_fixture();
        assert!(run(&store, &GraphQuery::CallersOf("nonexistent".into())).unwrap().is_empty());
    }
}

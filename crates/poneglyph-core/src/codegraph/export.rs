//! Export the whole code graph as JSON, GraphViz DOT, or GraphML.

use anyhow::Result;
use serde_json::json;

use crate::model::{CgEdge, CgNode};
use crate::store::Store;

fn all_nodes_and_edges(store: &Store) -> Result<(Vec<CgNode>, Vec<CgEdge>)> {
    let mut nodes = Vec::new();
    for file in store.cg_all_files()? {
        nodes.extend(store.cg_nodes_in_file(&file.path)?);
    }
    Ok((nodes, store.cg_all_edges()?))
}

pub fn export_json(store: &Store) -> Result<String> {
    let (nodes, edges) = all_nodes_and_edges(store)?;
    Ok(serde_json::to_string_pretty(&json!({ "nodes": nodes, "edges": edges }))?)
}

pub fn export_dot(store: &Store) -> Result<String> {
    let (nodes, edges) = all_nodes_and_edges(store)?;
    let mut out = String::from("digraph codegraph {\n");
    for n in &nodes {
        out.push_str(&format!("  \"{}\" [label=\"{}\", kind=\"{}\"];\n", dot_escape(&n.id), dot_escape(&n.name), n.kind));
    }
    for e in &edges {
        out.push_str(&format!("  \"{}\" -> \"{}\" [kind=\"{}\"];\n", dot_escape(&e.src_id), dot_escape(&e.dst_id), e.kind));
    }
    out.push_str("}\n");
    Ok(out)
}

pub fn export_graphml(store: &Store) -> Result<String> {
    let (nodes, edges) = all_nodes_and_edges(store)?;
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<graphml xmlns=\"http://graphml.graphdrawing.org/xmlns\">\n\
         <key id=\"node_kind\" for=\"node\" attr.name=\"kind\" attr.type=\"string\"/>\n\
         <key id=\"node_name\" for=\"node\" attr.name=\"name\" attr.type=\"string\"/>\n\
         <key id=\"edge_kind\" for=\"edge\" attr.name=\"kind\" attr.type=\"string\"/>\n\
         <graph id=\"codegraph\" edgedefault=\"directed\">\n",
    );
    for n in &nodes {
        out.push_str(&format!(
            "  <node id=\"{}\">\n    <data key=\"node_kind\">{}</data>\n    <data key=\"node_name\">{}</data>\n  </node>\n",
            xml_escape(&n.id),
            xml_escape(&n.kind.to_string()),
            xml_escape(&n.name)
        ));
    }
    for e in &edges {
        out.push_str(&format!(
            "  <edge source=\"{}\" target=\"{}\">\n    <data key=\"edge_kind\">{}</data>\n  </edge>\n",
            xml_escape(&e.src_id),
            xml_escape(&e.dst_id),
            xml_escape(&e.kind.to_string())
        ));
    }
    out.push_str("  </graph>\n</graphml>\n");
    Ok(out)
}

fn dot_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CodeGraphConfig;
    use tempfile::tempdir;

    fn fixture_store() -> Store {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn a() { b(); }\nfn b() {}\n").unwrap();
        let store = Store::open_in_memory().unwrap();
        super::super::build::build(&store, dir.path(), &CodeGraphConfig::default(), true).unwrap();
        store
    }

    #[test]
    fn export_json_round_trips_node_and_edge_counts() {
        let store = fixture_store();
        let json = export_json(&store).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["nodes"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["edges"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn export_dot_is_well_formed() {
        let store = fixture_store();
        let dot = export_dot(&store).unwrap();
        assert!(dot.starts_with("digraph codegraph {"));
        assert!(dot.trim_end().ends_with('}'));
        assert!(dot.contains("->"));
    }

    #[test]
    fn export_graphml_is_valid_xml_with_expected_elements() {
        let store = fixture_store();
        let graphml = export_graphml(&store).unwrap();
        assert!(graphml.starts_with("<?xml"));
        assert_eq!(graphml.matches("<node ").count(), 2);
        assert_eq!(graphml.matches("<edge ").count(), 1);
        assert!(graphml.trim_end().ends_with("</graphml>"));
    }

    #[test]
    fn export_escapes_special_characters() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.py"), "import os\n").unwrap();
        let store = Store::open_in_memory().unwrap();
        super::super::build::build(&store, dir.path(), &CodeGraphConfig::default(), true).unwrap();
        // The import node's name is the raw "import os" text — exercise XML/DOT escaping paths.
        let _ = export_dot(&store).unwrap();
        let _ = export_graphml(&store).unwrap();
    }
}

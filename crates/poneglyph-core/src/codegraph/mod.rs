//! Code knowledge graph (Tree-sitter). Distinct from the memory-linkage
//! edges in `crate::graph` / `[memory.edges]` — this module parses source
//! files into a structural graph of functions/methods/types/imports/tests,
//! stored in the `cg_*` tables (see `store.rs`).

pub mod blast_radius;
pub mod build;
pub mod export;
pub mod parse;
pub mod query;

pub use blast_radius::{BlastRadiusReport, DependentNode, blast_radius};
pub use build::{BuildReport, build};
pub use export::{export_dot, export_graphml, export_json};
pub use parse::language_for_extension;
pub use query::{GraphQuery, parse_query, run as run_query};

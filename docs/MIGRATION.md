# Migration Guide

Schema version is tracked in `schema_meta.schema_version` and checked on
every `Store::open` — migrations run automatically and in order, no manual
step beyond starting `poneglyph` (`init`/`mcp`/any CLI command). Each
step below is additive; no destructive changes have been made to any
existing column or table.

## v3 → v4 — compression cache

Added (`crates/poneglyph-core/src/store.rs:163-166`):

```sql
ALTER TABLE memories ADD COLUMN compressed_content TEXT;
ALTER TABLE memories ADD COLUMN compression_mode    TEXT;
```

`compressed_content` is a cache for context injection only —
recall/FTS/vector search never read it, and `content` itself is never
overwritten. See [COMPRESSION.md](COMPRESSION.md).

## v2 → v3 — code knowledge graph

Added `cg_files`, `cg_nodes`, `cg_edges` tables (Tree-sitter code graph;
distinct from the memory-linkage `edges` table from v1). See
[CODEGRAPH.md](CODEGRAPH.md).

## v1 → v2 — schema decoys

Added `memories.is_decoy` / `tier` / `strength` / `cold_path` columns and
the `decoy_children` table, supporting `poneglyph consolidate` /
`poneglyph decay`.

## v0 → v1 (Genesis)

**v1 is the first release.** There is no v0 schema to migrate from — all prior development used schema version 1, which is the DDL shipped in `poneglyph-core/src/store.rs`.

### Schema version tracking

The `schema_meta` table stores a `schema_version` row. The binary checks this on startup:

- If absent or `0` → runs the full DDL v1 (creates all tables, indexes, virtual tables).
- If `1` → no migration needed.
- If future versions are introduced, ordered migrations will run within a transaction.

### What is created (v1)

| Object | Purpose |
|---|---|
| `memories` | Core memory storage (content, type, importance, project, metadata) |
| `vec_memories` | sqlite-vec 384-dim vector index for dense KNN |
| `fts_memories` | FTS5 full-text search index |
| `edges` | Knowledge graph edges (explicit, similarity, temporal, tag_overlap, relation) |
| `projects` | Detected/registered projects (path + git remote identity) |
| `jobs` | Background enrichment job queue |
| `schema_meta` | Schema version tracking |

### Embedding dimensions

v1 hardcodes **384 dimensions**. `poneglyph init` offers a choice of 384d
models (see [INSTALL.md](INSTALL.md)), but switching to a model of a
*different* dimensionality requires a full re-embed migration — not
supported automatically. You would need to:

1. Stop `poneglyph mcp` and `poneglyph viewer` (both touch the same DB)
2. Delete the `vec_memories` table
3. Update `embedding.model_id` in config
4. Run a re-embed script (not yet provided)

**Do not mix dimensions** — sqlite-vec will silently return wrong results.

### Backup before upgrading

Before upgrading to a future version, back up the database:

```sh
cp ~/.local/share/poneglyph/poneglyph.db ~/.local/share/poneglyph/poneglyph.db.bak
```

The WAL journal files (`.db-wal`, `.db-shm`) should also be copied if present.

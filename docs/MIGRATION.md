# Migration Guide

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

v1 hardcodes **384 dimensions** (matching `BAAI/bge-small-en-v1.5`). Changing the embedding model to a different dimensionality requires a full re-embed migration. This is not supported automatically — you would need to:

1. Stop `poneglyph serve`
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

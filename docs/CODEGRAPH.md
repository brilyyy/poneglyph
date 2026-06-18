# Code Knowledge Graph

Tree-sitter–based structural graph of your codebase — distinct from the
memory graph (which links *memories* to each other). Tracks functions,
methods, types, imports, and tests, plus call/import/test-coverage edges
between them. Powers `callers_of` / `blast-radius` style impact analysis.

## Build the graph

```sh
# Full build: parse every matching file under <path> (default ".")
poneglyph graph init [path]

# Incremental: only reparse files whose content changed since last build
poneglyph graph update [path]

# Watch <path> and incrementally rebuild on change (debounced)
poneglyph graph watch [path]
```

Languages auto-detected by extension: rust, typescript, javascript, python, go.
Per-file parsing runs in parallel (rayon); DB writes stay serial since the
build shares one connection.

## Query the graph

```sh
# Structured queries
poneglyph graph query "callers_of:build_exclude_matcher"
poneglyph graph query "callees_of:build_exclude_matcher"
poneglyph graph query "imports_of:build_exclude_matcher"
poneglyph graph query "tests_for:build_exclude_matcher"

# Shortest call/import chain between two symbols
poneglyph graph query "path:cmd_init..install_skill_file"

# Bare keyword falls back to name search
poneglyph graph query "exclude_matcher"

# Recursive caller/importer/test trace ("what breaks if I change this")
poneglyph graph blast-radius <file-or-symbol> [--depth N]

# Export
poneglyph graph export --format json|dot|graphml [--out path]
```

`--depth` defaults to `[graph].blast_radius_depth` (default `5`).

## `.poneglyphignore`

Place a `.poneglyphignore` file at the project root to exclude files from
the graph build. Syntax is gitignore-style (negation, directory anchoring,
etc. — parsed via the `ignore` crate).

- Merges with `[graph].exclude_patterns` in config (glob syntax) — either
  source can exclude a path; neither overrides the other into inclusion.
- Deliberately does **not** honor `.gitignore`, `.git/info/exclude`, or your
  global gitignore, so an existing repo's `.gitignore` doesn't silently
  change what gets graphed.

```toml
[graph]
enabled = true
exclude_patterns = ["**/target/**", "**/node_modules/**", "**/.git/**", "**/*.test.ts", "**/*_test.rs"]
watch_delay_ms = 2000
blast_radius_depth = 5
max_render_nodes = 50000
```

## MCP tools

| Tool | Params | Description |
|---|---|---|
| `codegraph_query` | `query: String` | `callers_of:<name>`, `callees_of:<name>`, `imports_of:<name>`, `tests_for:<name>`, `path:<a>..<b>` (shortest call/import chain between two symbols), or a bare keyword search. Requires `poneglyph graph init` to have been run. |
| `codegraph_blast_radius` | `target: String`, `depth?: usize` | Recursive caller/importer/test trace from a file or symbol — what breaks if this changes. `depth` defaults to `[graph].blast_radius_depth`. |

Response shapes:

```rust
struct CodegraphNodeView { id, kind /* function|method|type|import|test */, name, file_path, start_line, end_line }
struct CodegraphQueryResponse { results: Vec<CodegraphNodeView> }
struct CodegraphDependentView { node: CodegraphNodeView, depth: usize }
struct CodegraphBlastRadiusResponse { root: Vec<CodegraphNodeView>, dependents: Vec<CodegraphDependentView>, tests: Vec<CodegraphNodeView> }
```

## Dashboard

| Route | Backing API | Purpose |
|---|---|---|
| `/codegraph` | `GET /api/codegraph?focus=&depth=&limit=` | GPU-rendered (cosmos.gl/WebGL) graph viewer, with a render-limit slider. With `focus`, centers on a blast-radius trace; without it, samples up to `limit` nodes (default 500, capped by `[graph].max_render_nodes`, default 50000). Returns `{ nodes, edges, total_nodes, total_edges }` — the totals are exact regardless of `limit`, so the UI can show "showing X of Y (sampled)". |
| — | `GET /api/codegraph/stats` | `{ files, nodes, edges }` counts. |
| `/token-savings` | `GET /api/token-savings` | Samples up to 200 memories and estimates caveman-compression savings: `{ sampled_memories, original_bytes, compressed_bytes, savings_pct, compression_enabled }`. |
| `/status` | `GET /api/agents-status` | Per-agent wiring status (config flag + detected install dir) for `claude_code`, `cursor`, `gemini_cli`, `opencode`, `codex`, `copilot_cli`. |

## See also

- [COMPRESSION.md](COMPRESSION.md) — the memory-compression pipeline that `/api/token-savings` reports on
- [INTEGRATIONS.md](INTEGRATIONS.md) — MCP tool setup

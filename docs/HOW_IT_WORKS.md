# How Poneglyph Works

Poneglyph is a local memory system for coding agents. It stores, indexes, and retrieves context across sessions — all on your machine, no cloud required.

## The Big Picture

```
┌─────────────────────────────────────────────────────────┐
│  Coding Agent (Claude Code, Cursor, OpenCode, etc.)     │
│  └─ Hooks capture prompts, responses, tool usage        │
└──────────────────┬──────────────────────────────────────┘
                   │ MCP protocol (stdio)
┌──────────────────▼──────────────────────────────────────┐
│  MCP Server (8 tools)                                   │
│  remember · recall · forget · update_memory             │
│  get_project_context · list_memories                    │
│  codegraph_query · codegraph_blast_radius               │
└──────────────────┬──────────────────────────────────────┘
                   │
┌──────────────────▼──────────────────────────────────────┐
│  Core Engine (SQLite + local embeddings)                │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐              │
│  │ Memory   │  │ Code     │  │ Decay &  │              │
│  │ Store    │  │ Graph    │  │ Compress │              │
│  └──────────┘  └──────────┘  └──────────┘              │
└─────────────────────────────────────────────────────────┘
```

## Memory System

### How memories are created

Every interaction with your coding agent gets captured:

1. **User prompts** → stored as `episodic` memories (via `userpromptsubmit` hook)
2. **Assistant responses** → stored as `episodic` memories (via `stop` hook)
3. **Tool usage** (Edit, Write, Bash, etc.) → stored as `code_context` memories (via `posttooluse` hook)
4. **Manual** → `poneglyph remember "fact"` or MCP `remember` tool

Each memory gets:
- An **embedding** — a 384-dimensional vector from `all-MiniLM-L6-v2` (runs locally on CPU, ~30ms per batch)
- **FTS5 index** — full-text search with porter stemming
- **Edges** — connections to related memories (see below)

### How memories are retrieved

When you ask poneglyph to recall something, it runs a **three-path hybrid search**:

1. **Dense search** — cosine similarity against the embedding vectors (finds semantically similar content)
2. **Sparse search** — FTS5 keyword matching (finds exact terms)
3. **Graph expansion** — follows 1-hop edges from top results to find connected memories

All three paths are fused with **Reciprocal Rank Fusion** (RRF, k=60), then scored by:
- Relevance (the RRF score)
- Importance (0.0–1.0, set at creation or by enrichment)
- Recency (30-day exponential decay)
- Ebbinghaus strength (access-based forgetting curve)

### Memory tiers and decay

Memories move through tiers based on access patterns:

| Tier | Retention | What lives here |
|------|-----------|-----------------|
| Ephemeral | Session only | Current session context |
| Short-term | 7 days | Recent work, active debugging |
| Working | 30 days | Project context, decisions |
| Long-term | 180 days | Rarely accessed but valuable |
| Archival | Forever | Explicitly important memories |

Without access, memories lose ~2% strength per day (Ebbinghaus curve). When strength drops below 0.3, memories become candidates for **consolidation**.

### Consolidation pipeline (memory_type promotion)

Separately from the storage tiers above, a pipeline promotes memories through
`memory_type` stages — `episodic` → `semantic` → `procedural` — runnable with
or without a local LLM:

| Stage | LLM path | Deterministic fallback (`llm.enabled=false`) |
|---|---|---|
| raw → episodic | Abstractive session summary | Extractive top-content join |
| episodic → semantic | Fact distillation + confidence score | Embedding-cluster decoy + cluster-cohesion confidence |
| semantic → procedural | Workflow synthesis (trigger/steps/outcome) | Frequent tool-sequence n-gram mining |

Every promoted memory keeps **lineage** back to its sources (so a `semantic`
decoy links to the `episodic` memories it was distilled from, and so on).
Runs on two triggers: the `poneglyph mcp` daemon's scheduler
(`[consolidation] interval_hours`, default 6h) and a debounced call from the
Claude Code `stop` hook — see [INTEGRATIONS.md](INTEGRATIONS.md). Trigger it
manually with `poneglyph consolidate [--project <path>]`.

### Edges (the knowledge graph)

Memories connect to each other through edges:

| Edge type | How it's created | Example |
|-----------|-----------------|---------|
| Similarity | Embedding cosine ≥ 0.82 | Two memories about the same API |
| Temporal | Same project, ≤ 5 min apart | Memories from the same debugging session |
| Tag overlap | Shared tags (Jaccard ≥ 0.5) | Both tagged "authentication" |
| Explicit | Manual link | `remember` with `related_to` |
| Relation | LLM-extracted | "depends on", "fixes" |

These edges power graph expansion during retrieval — finding related context you didn't explicitly search for.

### Compression

When `compression_enabled = true`, memories are compressed with "caveman grammar":
- Common English words → single Unicode codepoints (~40% size reduction)
- Code, paths, versions, identifiers are never touched
- Fully reversible: `expand(compress(x)) == x`

Old memories get **cold storage**: zstd-compressed files on disk, decompressed on demand.

## Code Graph

The code graph is a separate index of your source code, built with tree-sitter parsers.

### What it tracks

For each source file, poneglyph extracts:
- **Functions and methods** — name, file, line number
- **Types** — structs, classes, interfaces, enums
- **Imports** — what each file depends on
- **Calls** — who calls whom
- **Tests** — test functions and what they test

### How it's built

```
poneglyph graph init          # First build: parse all source files
poneglyph graph update        # Incremental: only changed files (by content hash)
poneglyph graph watch         # File watcher: auto-rebuild on save (debounced 2s)
```

The build process:
1. Walk the project directory (honoring `.poneglyphignore` + config `exclude_patterns`)
2. Parse each file with the matching tree-sitter grammar (parallel via rayon)
3. Store nodes (`cg_nodes`) and edges (`cg_edges`) in SQLite
4. Resolve call edges in a second pass (needs all nodes first)

### Supported languages

Currently: Rust, TypeScript, JavaScript, Python, Go. Each language defines:
- Which tree-sitter node kinds are functions, types, imports, calls
- How to detect tests (attributes, naming conventions)

### Queries

```
callers_of:parse_config      # Who calls this function?
callees_of:main              # What does this function call?
imports_of:store.rs          # What does this file import?
tests_for:retrieve           # What tests cover this code?
path:main.rs..store.rs       # Shortest call/import chain between two files
```

Blast radius: "If I change X, what breaks?" — recursive BFS through callers and importers.

## MCP Server

Poneglyph exposes 8 tools via the Model Context Protocol (stdio transport):

| Tool | What it does |
|------|-------------|
| `remember` | Store a memory (auto-embeds, auto-indexes) |
| `recall` | Hybrid search (dense + FTS + graph) |
| `forget` | Delete a memory by ID |
| `update_memory` | Replace content, re-embed |
| `get_project_context` | Ranked context string for session injection |
| `list_memories` | Paginated listing with filters |
| `codegraph_query` | Structured code graph queries |
| `codegraph_blast_radius` | "What breaks if I change this?" |

The key design: embedding runs **before** locking the store, so no async happens under the mutex. Edge computation is always enqueued as a background job.

## Hooks

Hooks are thin shell scripts that call the `poneglyph` CLI. They're registered in your agent's settings file.

### Claude Code hooks

| Hook | When | What it stores |
|------|------|----------------|
| `sessionstart.sh` | Session begins | Injects project context (read-only) |
| `userpromptsubmit.sh` | User sends a message | Stores the prompt as `episodic` memory |
| `posttooluse.sh` | After tool execution | Stores tool usage as `code_context`, triggers graph update on file edits |
| `stop.sh` | Session ends | Stores last assistant message as `episodic` memory |

All hooks run in background (`&`) to avoid blocking the agent. Timeouts are set to 5–10 seconds.

### OpenCode plugin

A TypeScript plugin that uses MCP directly (no shell hooks). Same capture logic, different transport.

## Web Dashboard

The viewer is a React SPA embedded in the poneglyph binary (served at `http://localhost:3742`).

| Page | What it shows |
|------|--------------|
| Dashboard | Memory/project counts, recent activity |
| Memories | Browse, search, edit, delete memories |
| Timeline | Session history with Q&A pairs |
| Search | Semantic search across all memories |
| Graph | WebGL visualization of memory connections |
| Code graph | WebGL visualization of code structure |
| Token savings | Compression statistics |
| Status | System health, embedding model, storage |
| Settings | Edit config.toml via UI |

The graph visualizations use [cosmos.gl](https://github.com/cosmograph/cosmos) for WebGL rendering — handles 50k+ nodes smoothly.

## Configuration

Config lives at `~/.config/poneglyph/config.toml` (global) or `.poneglyph/config.toml` (project-local). Local overrides global; arrays replace (not merge).

Key sections:

| Section | Controls |
|---------|----------|
| `[general]` | Data directory, log level |
| `[embedding]` | Model, dimensions, device |
| `[llm]` | Optional LLM for enrichment/summaries |
| `[memory]` | Retention tiers, compression, edges |
| `[graph]` | Languages, exclude patterns, build settings |
| `[dashboard]` | Port, host, auth token, theme |
| `[agents]` | Which agent hooks are active |
| `[privacy]` | Redaction tags, excluded paths |
| `[decay]` | Forgetting curve parameters |
| `[consolidation]` | Memory merging settings |
| `[cold_storage]` | zstd compression for old memories |

Supports `{ env.VAR_NAME }` interpolation for secrets. See `config.toml` comments for detailed descriptions of each setting.

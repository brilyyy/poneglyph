# poneglyph — Product Requirements Document (v1 Stable)

> **Status:** Draft for implementation
> **Target:** v1.0 stable release
> **Audience:** Implementing engineer / Claude Code
> **Format note:** This document is the authoritative spec. Sections marked **[MUST]** are release-blocking for v1. Sections marked **[SHOULD]** are strongly desired. **[DEFERRED]** items are explicitly out of scope for v1 and must not be built (only stubbed where noted).

---

## 1. Summary

`poneglyph` is a **desktop-first, local, AI-native memory engine** that gives any AI agent persistent memory across sessions. It ships as a **single Rust binary** with embedded storage, an MCP server, an HTTP API, and an embedded web viewer. It works **fully offline** and makes **zero LLM calls by default** (local embeddings + heuristics). Optional LLM enrichment can be enabled by the user, pointed at a local or remote OpenAI-compatible endpoint.

The core insight shaping the architecture: **MCP is the universal recall/capture substrate** (works with any MCP client — coding agents and chat AIs alike), and **passive session capture is an opportunistic enhancement layer** delivered via thin per-client hook adapters that POST to a local HTTP endpoint. These two paths are designed independently.

---

## 2. Problem Statement

AI agents are stateless across sessions. Today this is patched with ad-hoc context files, copy-paste, or cloud memory services that require trust, network access, and per-token cost. There is no good local-first option that:

1. Works offline with no forced token spend.
2. Integrates with *any* MCP-compatible agent without bespoke code.
3. Captures coding-session context automatically where the client allows it.
4. Ships as one binary with no Docker / Node / Python runtime to install.

`poneglyph` fills that gap.

---

## 3. Goals & Non-Goals

### 3.1 Goals (v1) **[MUST]**
- Store, retrieve, update, and delete memories via a stable API.
- Local embeddings with no network dependency; **0 tokens** in default mode.
- Hybrid retrieval: dense (vector) + sparse (keyword) + graph expansion.
- Knowledge-graph edges computed locally without an LLM.
- MCP server (stdio) exposing memory tools to any MCP client.
- HTTP API serving (a) the embedded viewer and (b) a passive-capture ingest endpoint.
- Project detection and context injection on project reopen.
- Optional, background, non-blocking LLM enrichment (off by default).
- An embedded web viewer for browsing, searching, and exploring the memory graph.
- Single distributable binary for macOS (arm64) and Linux (x86_64).

### 3.2 Non-Goals (explicitly **[DEFERRED]** — do not build in v1)
- Cloud / managed hosting tier, billing, multi-tenancy.
- Self-hosted PostgreSQL/pgvector backend (define the trait; ship only the SQLite impl).
- Multi-user accounts, auth, sharing, RBAC.
- Mobile clients.
- WebGPU / GPU-accelerated embeddings.
- gRPC / Protobuf / MessagePack transports.
- Reranker models (may be a **[SHOULD]** stretch; not blocking).

---

## 4. Users & Personas

| Persona | Need | Primary interface |
|---|---|---|
| **Solo dev (primary, "dogfood")** | Memory across coding sessions in Claude Code / opencode | MCP + passive hooks + viewer |
| **Chat-AI power user** | Persistent facts/preferences across chat sessions | MCP (explicit `remember`/`recall`) |
| **Self-hoster (future)** | Run for a small team | (deferred — trait only) |

v1 success is measured against the primary persona first.

---

## 5. Product Principles

1. **Token-safe by default.** No LLM call happens unless the user explicitly enables enrichment or passes `llm_assist: true`.
2. **Offline-first.** Every core feature works with no internet. Network is only touched for (a) optional LLM enrichment, (b) first-run model download.
3. **Single binary.** No Docker, Node, or Python at runtime. The viewer is compiled in.
4. **Never block the agent.** Embedding and enrichment that could add latency run off the request path.
5. **One file of truth.** All persistent state lives in a single SQLite database file plus a model cache directory.
6. **Pluggable, not premature.** Abstract storage behind a trait, but ship only what v1 needs.

---

## 6. System Architecture

### 6.1 High-level

```
                ┌──────────────────────────────────────────┐
   MCP clients  │                                          │
 (Claude Code,  │   poneglyph-mcp  ── stdio JSON-RPC (rmcp)  │
  Claude        │        │                                  │
  Desktop,      │        ▼                                  │
  opencode,     │   poneglyph-core  ── storage · embeddings  │
  Hermes, any   │        ▲          retrieval · graph ·     │
  MCP chat AI)  │        │          enrichment queue        │
                │   poneglyph-http ── axum (localhost)       │
 hook adapters  │        │   ├── /ingest  (passive capture) │
 (Claude Code   │        │   └── /api/*   (viewer backend)  │
  PostToolUse,  │        ▼                                  │
  opencode      │   Embedded viewer (TanStack Start)        │
  plugin)       │                                          │
                └──────────────────────────────────────────┘
                              │
                              ▼
                  ~/.local/share/poneglyph/poneglyph.db   (single SQLite file)
                  ~/.cache/poneglyph/models/             (embedding model cache)
```

### 6.2 Two front doors into one core
- **MCP server (`poneglyph-mcp`)** — the universal interface. Recall + explicit capture. Works with any MCP client. **stdio transport** for local single-user; Streamable HTTP transport is a future option, not v1.
- **HTTP server (`poneglyph-http`)** — two jobs: serve the viewer (`/api/*`) and receive passive capture events (`/ingest`).

Both call the same `poneglyph-core` library. No business logic lives in the adapters.

### 6.3 Capture model (critical design constraint)
MCP is **pull-based**: an MCP server cannot observe a session on its own; the client decides what to send. Therefore:
- **Explicit capture** = the agent calls the `remember` tool. Universal, but depends on the agent choosing to.
- **Passive capture** = a client-specific hook adapter POSTs session events to `/ingest`. Only available for clients with a hook/plugin system (Claude Code hooks, opencode plugins). **For chat AIs without hooks, only explicit capture is available — this is expected, not a bug.**

### 6.4 Workspace layout

```
poneglyph/
├── Cargo.toml                  # workspace (see companion cargo skeleton)
├── crates/
│   ├── poneglyph-core/          # storage, embeddings, graph, retrieval, enrichment
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── store/          # SQLite: memories, vec, fts, edges, projects, jobs
│   │   │   ├── embed/          # embed_anything wrapper (candle backend)
│   │   │   ├── retrieve/       # hybrid search + RRF fusion
│   │   │   ├── graph/          # no-LLM + optional-LLM edge builders
│   │   │   ├── enrich/         # background job queue + LLM client
│   │   │   ├── project/        # detection + context injection
│   │   │   └── model.rs        # Memory, Edge, Project, Job types
│   ├── poneglyph-mcp/           # rmcp stdio server, tool defs
│   ├── poneglyph-http/          # axum: /ingest + /api/*, rust-embed viewer
│   └── poneglyph-cli/           # binary `poneglyph`; wires it together
├── viewer/                     # TanStack Start app (built into poneglyph-http)
├── hooks/
│   ├── claude-code/            # PostToolUse / UserPromptSubmit hook scripts
│   └── opencode/               # plugin
├── tests/                      # integration + retrieval eval harness
└── docs/
    ├── PRD.md                  # this file
    ├── INSTALL.md
    └── INTEGRATIONS.md
```

---

## 7. Data Model

Single SQLite file. `rusqlite` with `bundled` (static-linked SQLite). Vectors via `sqlite-vec` (`vec0` virtual table). Keyword via built-in FTS5. Graph via a plain `edges` table walked with recursive CTEs.

> **Caveat:** `sqlite-vec` is pre-v1; pin the version and gate it behind the `Store` trait so it can be swapped for `libsql` native vectors if churn becomes painful. **[MUST]** isolate all vector-extension calls in one module.

### 7.1 Schema (DDL)

```sql
-- Core memory record
CREATE TABLE memories (
    id            TEXT PRIMARY KEY,          -- UUIDv7 (time-sortable)
    content       TEXT NOT NULL,
    memory_type   TEXT NOT NULL,             -- enum: episodic|semantic|procedural|fact|preference|code_context
    importance    REAL NOT NULL DEFAULT 0.5, -- 0.0–1.0
    project_id    TEXT,                       -- FK projects.id, nullable
    source        TEXT NOT NULL,             -- enum: explicit|passive|cli|import
    metadata      TEXT,                       -- JSON blob (tags, file paths, tool name, etc.)
    created_at    TEXT NOT NULL,             -- ISO-8601 UTC
    updated_at    TEXT NOT NULL,
    accessed_at   TEXT,                       -- last recall hit (for decay/ranking)
    access_count  INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE SET NULL
);
CREATE INDEX idx_mem_project ON memories(project_id);
CREATE INDEX idx_mem_type    ON memories(memory_type);
CREATE INDEX idx_mem_created ON memories(created_at);

-- Vector index (sqlite-vec). 384 dims fixed for v1 (matches bge-small-en-v1.5).
CREATE VIRTUAL TABLE vec_memories USING vec0(
    memory_id TEXT PRIMARY KEY,
    embedding FLOAT[384]
);

-- Keyword index (FTS5)
CREATE VIRTUAL TABLE fts_memories USING fts5(
    memory_id UNINDEXED,
    content,
    tokenize = 'porter unicode61'
);

-- Knowledge graph edges
CREATE TABLE edges (
    id          TEXT PRIMARY KEY,            -- UUIDv7
    src_id      TEXT NOT NULL,
    dst_id      TEXT NOT NULL,
    edge_type   TEXT NOT NULL,               -- enum: explicit|similarity|temporal|tag_overlap|relation
    label       TEXT,                         -- predicate text for LLM 'relation' edges; NULL otherwise
    weight      REAL NOT NULL DEFAULT 1.0,
    created_at  TEXT NOT NULL,
    FOREIGN KEY (src_id) REFERENCES memories(id) ON DELETE CASCADE,
    FOREIGN KEY (dst_id) REFERENCES memories(id) ON DELETE CASCADE
);
CREATE INDEX idx_edge_src ON edges(src_id);
CREATE INDEX idx_edge_dst ON edges(dst_id);
CREATE UNIQUE INDEX idx_edge_unique ON edges(src_id, dst_id, edge_type, COALESCE(label,''));

-- Projects
CREATE TABLE projects (
    id           TEXT PRIMARY KEY,           -- UUIDv7
    path         TEXT NOT NULL,              -- absolute directory path
    git_remote   TEXT,                        -- normalized git remote URL, nullable
    name         TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    last_seen_at TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_project_path ON projects(path);

-- Background enrichment jobs (durable queue)
CREATE TABLE jobs (
    id          TEXT PRIMARY KEY,            -- UUIDv7
    job_type    TEXT NOT NULL,               -- enum: summarize|extract_entities|extract_relations|score_importance
    memory_id   TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'pending', -- pending|running|done|failed
    attempts    INTEGER NOT NULL DEFAULT 0,
    last_error  TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    FOREIGN KEY (memory_id) REFERENCES memories(id) ON DELETE CASCADE
);
CREATE INDEX idx_jobs_status ON jobs(status);

-- Schema version for migrations
CREATE TABLE schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
-- seed: ('schema_version', '1')
```

### 7.2 Type definitions (Rust, in `model.rs`)

```rust
pub enum MemoryType { Episodic, Semantic, Procedural, Fact, Preference, CodeContext }
pub enum Source     { Explicit, Passive, Cli, Import }
pub enum EdgeType   { Explicit, Similarity, Temporal, TagOverlap, Relation }
pub enum JobType    { Summarize, ExtractEntities, ExtractRelations, ScoreImportance }
```

### 7.3 Migrations **[MUST]**
- A `schema_meta.schema_version` row gates startup migrations.
- On startup, if the file's version < binary's expected version, run ordered migrations inside a transaction.
- v1 ships version `1`. Changing embedding dimensions is a **breaking** migration that requires re-embedding all rows — document this and refuse to silently mix dimensions.

---

## 8. Functional Requirements

Each feature lists acceptance criteria (AC) that double as test targets.

### 8.1 Memory CRUD **[MUST]**
- **Create:** store content + type + importance + optional project + metadata; generate embedding; index into `vec_memories` and `fts_memories`; enqueue no-LLM edge computation.
- **Read:** fetch by id; list with filters (type, project, date range, tag); paginate.
- **Update:** edit content → re-embed, re-index, recompute edges; bump `updated_at`.
- **Delete:** cascade removes vec row, fts row, and edges.
- **AC1:** Creating a memory makes it retrievable by semantic `recall` within the same process without restart.
- **AC2:** Deleting a memory removes all associated vec/fts/edge rows (verified by row counts).
- **AC3:** Updating content changes the stored embedding (verified by vector inequality).

### 8.2 Embedding pipeline **[MUST]**
- Use `embed_anything` with the **candle** backend (no ONNX runtime, no PyTorch).
- Default model: `BAAI/bge-small-en-v1.5` (384d). Model id is configurable; dimension is fixed at 384 for v1.
- First run downloads the model to the cache dir with a visible progress indicator; subsequent runs are offline.
- Embedding runs on a dedicated thread/pool; the `remember` call returns after persistence, and embedding is awaited before indexing but must not block unrelated requests.
- **AC1:** With the model cached, the process starts and serves `recall` with **no network access**.
- **AC2:** Swapping `model_id` to another 384d model in config works without code changes.

### 8.3 Hybrid retrieval **[MUST]**
- `recall(query, filters, limit)` runs three retrievers and fuses them:
  1. **Dense:** embed query → `vec_memories` KNN (brute-force is acceptable at v1 scale).
  2. **Sparse:** FTS5 MATCH on `fts_memories`.
  3. **Graph expansion:** for top dense/sparse hits, pull 1-hop neighbors via `edges`.
- Fuse with **Reciprocal Rank Fusion (RRF)**, then apply a light recency/importance boost. Return top `limit`.
- Update `accessed_at` and `access_count` on returned memories.
- **AC1:** A query matching on a synonym (not exact keyword) returns the relevant memory via the dense path.
- **AC2:** A query with a rare exact token (e.g., an error code) returns it via the sparse path even if dense misses.
- **AC3:** Retrieval over 100k memories returns within the latency target in §11.

### 8.4 Knowledge graph — no-LLM edges **[MUST]**
Computed locally, always, with zero tokens:
- **Explicit:** caller-provided links between memory ids.
- **Similarity:** cosine similarity above a configurable threshold (default 0.82) between embeddings.
- **Temporal:** memories created within a configurable window (default 5 min) in the same project.
- **Tag overlap:** shared tags/metadata keys above a threshold.
- Edge computation is enqueued and runs on the background worker; it must never block `remember`.
- **AC1:** Two near-duplicate memories produce a `similarity` edge.
- **AC2:** Edges respect the unique constraint (no duplicates on recompute).

### 8.5 Knowledge graph — optional LLM edges **[SHOULD]**
- When enrichment is enabled, an `extract_relations` job produces labeled subject–predicate–object `relation` edges.
- Off by default; gated by config and/or per-call `llm_assist`.

### 8.6 MCP server **[MUST]**
- Implemented with `rmcp`, stdio transport, tools defined via the `#[tool]` macro.
- Exposes the tools in §9. Tool handlers call `poneglyph-core`; no heavy work on the handler thread.
- **AC1:** A round-trip from Claude Code (configured as an MCP server) successfully stores and recalls a memory.
- **AC2:** The same binary works as an MCP server in Claude Desktop.

### 8.7 HTTP server — ingest **[MUST]**
- `POST /ingest` accepts a session event (schema §10.2) and creates a `passive`-source memory (+ enqueues edges/enrichment).
- Bound to `127.0.0.1` by default. If bound to a non-loopback address, an API token is **required** (§12).
- **AC1:** A Claude Code `PostToolUse` hook posting JSON produces a stored memory tagged with the tool name and project.

### 8.8 HTTP server — viewer API **[MUST]**
- `GET /api/memories` (filter/paginate), `GET /api/memories/:id`, `PATCH`, `DELETE`.
- `GET /api/search?q=` (hybrid recall).
- `GET /api/graph?focus=:id&depth=n` (nodes + edges for the explorer).
- `GET /api/projects`, `GET /api/settings`, `PATCH /api/settings`.
- `GET /api/stats` (counts for dashboard).

### 8.9 Passive capture hook adapters **[MUST for Claude Code, SHOULD for opencode]**
- **Claude Code:** ship `hooks/claude-code/` with a `PostToolUse` (and optional `UserPromptSubmit`) hook that curls JSON to `/ingest`, plus install instructions in `INTEGRATIONS.md`.
- **opencode:** ship a plugin in `hooks/opencode/` performing the same POST. (SHOULD; not release-blocking.)
- Adapters are thin shims — **no business logic**.

### 8.10 Project detection & context injection **[MUST]**
- Detect project by absolute directory path; if a git remote exists, normalize and store it for stable identity across clones.
- On `get_project_context(project_path, max_tokens)`: assemble a string from the project's most relevant memories (ranked by importance × recency × access), truncated to a token budget (default 2000, configurable). Return it for the agent to inject into its system prompt.
- New project ⇒ created with empty memory; no file scanning in v1 (**[DEFERRED]**).
- **AC1:** Reopening a known project yields a non-empty context string under the token budget.
- **AC2:** Identical repo cloned to a new path is recognized as the same project via git remote.

### 8.11 Optional LLM enrichment **[SHOULD]**
- Background worker drains the `jobs` table. Job types: `summarize`, `extract_entities`, `extract_relations`, `score_importance`.
- Uses `async-openai` against a configurable OpenAI-compatible endpoint (Ollama, llama.cpp server, or remote).
- Disabled by default. Failures retry with backoff up to a cap, then mark `failed` and move on (never crash, never block).
- **AC1:** With enrichment off, the `jobs` table only contains no-LLM edge work and the LLM client is never constructed.
- **AC2:** With enrichment on and the endpoint unreachable, jobs fail gracefully and the rest of the system is unaffected.

### 8.12 Viewer (TanStack Start) **[MUST]**
- Pages: **Dashboard** (stats), **Memories** (list + filters), **Memory detail** (content, metadata, edges), **Search**, **Graph explorer** (React Flow, force-directed; node color by `memory_type`; edge filters by `edge_type`; click to expand neighborhood), **Settings**.
- Built to static assets and embedded via `rust-embed`; served by `poneglyph-http`. No separate Node runtime at run time.
- **AC1:** `poneglyph serve` then visiting `http://localhost:<port>` loads the dashboard with live data.
- **AC2:** Graph explorer renders 500 nodes and supports neighborhood expansion.

### 8.13 CLI **[MUST]**
- `poneglyph init` — create db + default config.
- `poneglyph serve` — run MCP stdio server **and** HTTP server concurrently.
- `poneglyph remember "<text>" [--type --importance --project --tag]`
- `poneglyph recall "<query>" [--limit]`
- `poneglyph forget <id>`
- `poneglyph export [--format json|md]`
- `poneglyph status` — db path, counts, model, enrichment on/off.

### 8.14 Configuration **[MUST]**
- TOML at the platform config dir (e.g. `~/.config/poneglyph/config.toml`), overridable by env vars (`figment`).
- Fields: `db_path`, `model_cache_dir`, `embedding.model_id`, `server.mcp` (on/off), `server.http_port`, `server.bind_addr`, `server.api_token`, `llm.enabled`, `llm.endpoint`, `llm.model`, `enrichment.*` toggles, `graph.similarity_threshold`, `graph.temporal_window_secs`, `context.max_tokens`.

---

## 9. MCP Tool Specifications

All tools are namespaced under the server. Inputs/outputs are JSON.

```
remember
  in:  { content: string,
         memory_type?: "episodic"|"semantic"|"procedural"|"fact"|"preference"|"code_context",
         importance?: number(0..1),
         project_path?: string,
         tags?: string[],
         llm_assist?: boolean }            // default false
  out: { id: string }

recall
  in:  { query: string,
         limit?: number,                   // default 10
         memory_type?: string,
         project_path?: string,
         since?: string }                  // ISO-8601
  out: { results: [ { id, content, memory_type, importance, score, created_at, metadata } ] }

forget
  in:  { id: string }
  out: { deleted: boolean }

update_memory
  in:  { id: string, new_content: string }
  out: { id: string, updated: boolean }

get_project_context
  in:  { project_path: string, max_tokens?: number }   // default 2000
  out: { context: string, memory_count: number }

list_memories
  in:  { project_path?: string, memory_type?: string, limit?: number, offset?: number }
  out: { results: [...], total: number }
```

**Tool behavior rules:** no tool blocks on embedding/enrichment beyond what is required to persist and index; `llm_assist:true` enqueues enrichment, it does not run it synchronously.

---

## 10. HTTP API & Ingest Specifications

### 10.1 Viewer/API endpoints
See §8.8. All return JSON. Errors use standard HTTP status + `{ error: string }`.

### 10.2 Ingest event schema (`POST /ingest`)

```json
{
  "event": "tool_use | user_message | assistant_message | file_edit | terminal",
  "client": "claude-code | opencode | custom",
  "project_path": "/abs/path",            // optional but recommended
  "content": "string (the captured text)",
  "tool": "Bash | Edit | ...",            // optional, for tool_use events
  "metadata": { "any": "json" },           // optional
  "timestamp": "ISO-8601"                  // optional; server fills if absent
}
```
Server maps the event to a `code_context` (or `episodic`) memory with `source = passive`, attaches the project, and enqueues edge/enrichment work.

---

## 11. Non-Functional Requirements

Targets are for a modern laptop (e.g., Apple Silicon M-series) at **100k memories**, single user. These are realistic and testable — not the fictional sub-millisecond targets of the original design; the real cost center is embedding inference, which dominates write/query latency.

| Metric | Target | Notes |
|---|---|---|
| `recall` end-to-end (incl. query embed) p95 | < 150 ms | embedding inference dominates |
| `recall` retrieval-only (excluding embed) p95 | < 30 ms | brute-force vec scan + FTS + fusion |
| `remember` (excluding embed) p95 | < 50 ms | persist + index |
| Graph load (500 nodes) | < 100 ms | API + serialization |
| Viewer initial load | < 500 ms | embedded assets |
| Idle memory footprint | < 150 MB | model loaded |
| Cold start (model cached) | < 3 s | including model load |

- **Reliability [MUST]:** no operation may lose or corrupt data; all multi-write operations are transactional. A crash mid-enrichment leaves the DB consistent (jobs are idempotent / resumable).
- **Offline [MUST]:** all NFR targets are met with no network.
- **Portability [MUST]:** static binary; no system ONNX/Python deps (guaranteed by the candle backend choice).

---

## 12. Security & Privacy

- **Local by default [MUST]:** HTTP server binds `127.0.0.1`. MCP is stdio (no network).
- **Token gate [MUST]:** if `server.bind_addr` is non-loopback, startup requires a non-empty `server.api_token`; all `/api/*` and `/ingest` requests must present it (bearer header). Refuse to start otherwise.
- **No telemetry [MUST]:** the binary makes no analytics/phone-home calls. The only outbound network is optional model download and optional LLM enrichment to the user-configured endpoint.
- **At-rest [SHOULD]:** create the DB file with user-only permissions (0600). Full at-rest encryption is **[DEFERRED]**.
- **Secrets:** API tokens and LLM keys come from env/config, never logged.

---

## 13. Tech Stack (locked for v1)

| Concern | Choice |
|---|---|
| Language / runtime | Rust + Tokio |
| MCP | `rmcp` (official SDK, 1.x), stdio transport |
| HTTP | `axum` (+ `tower-http`); JSON only, no gRPC |
| Embeddings | `embed_anything` (candle backend), model `BAAI/bge-small-en-v1.5` (384d) |
| Storage | `rusqlite` (bundled) + `sqlite-vec` + FTS5 + `edges` table; async via `deadpool-sqlite` |
| ANN escape hatch | `hnsw_rs` / `instant-distance` only if brute force misses targets |
| Cache | `moka` |
| Job queue | tokio mpsc (phase 1) → `apalis` (SQLite store) for durable jobs |
| LLM client (optional) | `async-openai` (OpenAI-compatible endpoints) |
| Viewer | TanStack Start + React + shadcn/ui + React Flow; embedded via `rust-embed` |
| Serialization | `serde` / `serde_json` |
| Errors / logging | `anyhow` + `thiserror`; `tracing` |
| CLI / config | `clap`; `figment` (TOML + env) |
| IDs / time | `uuid` v7; `chrono` |

> The companion `poneglyph_cargo_skeleton.toml` is the canonical dependency manifest.

---

## 14. Milestones to v1 Stable

Phased for a part-time (nights/weekends) cadence. Estimates are rough and assume ~6–10 productive hours/week; treat ordering as firm, dates as soft.

| Phase | Deliverable | Exit criteria |
|---|---|---|
| **M0 — Skeleton** | Workspace, crates, config, `init`, migrations | `poneglyph init` creates db + config; CI builds on macOS+Linux |
| **M1 — Core + CLI** | Store, embed (candle), CRUD, hybrid `recall` | `remember`/`recall`/`forget` work via CLI; AC §8.1–8.3 pass |
| **M2 — MCP (thin slice)** | `rmcp` stdio server with the 6 tools | Claude Code round-trip store+recall works (AC §8.6) — **this validates the product** |
| **M3 — Graph (no-LLM)** | similarity/temporal/tag/explicit edges + background worker | AC §8.4 pass; edges visible via API |
| **M4 — HTTP + ingest + hooks** | axum `/ingest` + `/api/*`; Claude Code hook adapter | passive capture from a Claude Code hook works (AC §8.7, §8.9) |
| **M5 — Viewer** | TanStack Start app embedded; all pages | viewer loads with live data; graph explorer works (AC §8.12) |
| **M6 — Enrichment + project context** | optional LLM jobs; `get_project_context` | enrichment on/off behaves per §8.11; context injection per §8.10 |
| **M7 — Hardening / release** | perf pass, migrations, docs, packaging | §15 release criteria met |

---

## 15. Definition of v1 Stable (Release Criteria) **[MUST all]**

1. All **[MUST]** functional requirements implemented with their ACs passing.
2. MCP server verified working in **Claude Code** and **Claude Desktop**.
3. Passive capture verified via a **Claude Code hook**.
4. Embedded **viewer** functional across all listed pages.
5. NFR targets in §11 met at 100k memories on the reference machine.
6. **No data-loss or corruption** bugs; all writes transactional; crash-consistent.
7. Schema **migrations** present and tested (v0→v1 path documented even if v1 is genesis).
8. Test coverage: unit tests on `poneglyph-core` (store, retrieve, graph), integration tests for MCP round-trip and HTTP, and a small **retrieval-quality eval** (labeled set; report recall@10).
9. Cross-platform release binaries: **macOS arm64** and **Linux x86_64**.
10. Docs: `INSTALL.md`, `INTEGRATIONS.md` (Claude Code + Claude Desktop setup), and a README quickstart.
11. Runs **fully offline** after first-run model download.

---

## 16. Testing Strategy

- **Unit (core):** store CRUD, RRF fusion correctness, edge builders (deterministic given fixtures), migration runner.
- **Integration:** spin up the MCP server in-process and drive it with an `rmcp` client; hit the HTTP API with a test client; assert DB side effects.
- **Retrieval eval harness:** a small labeled corpus (queries → expected memory ids); compute recall@k and guard against regressions in CI.
- **Benchmarks:** seed 100k synthetic memories; measure §11 metrics with `criterion` or a custom harness; fail CI if a target regresses beyond a margin.
- **Failure injection:** unreachable LLM endpoint, corrupt job rows, missing model cache → assert graceful degradation.

---

## 17. Open Questions / Deferred

- **Cloud tier & self-hosted Postgres:** out of scope; only the `Store` trait boundary is built so they can be added later without core rewrites.
- **Reranker model** for retrieval quality: optional stretch; `embed_anything` supports rerankers, so it can be added behind a config flag.
- **File scanning on new project init:** deferred; v1 starts projects empty.
- **Streamable HTTP MCP transport:** add only when remote/multi-user is on the table (note: the MCP spec is moving toward a stateless protocol layer — revisit transport choices then).
- **At-rest encryption:** deferred to a later minor version.
- **Embedding dimension changes:** require full re-embed; design the migration story before ever shipping a non-384d default.

---

## 18. Appendix — Reference Flows

**A. Explicit capture (any MCP client)**
`agent calls remember(content)` → core persists → embed → index (vec+fts) → enqueue no-LLM edges → return `id`. No tokens spent.

**B. Passive capture (Claude Code)**
`PostToolUse hook` → `curl POST /ingest {event,tool,content,project_path}` → core creates `passive` `code_context` memory → enqueue edges (+ enrichment if enabled).

**C. Project reopen**
`agent calls get_project_context(path)` → core resolves project (path → git remote) → rank memories (importance×recency×access) → assemble ≤ max_tokens string → return for system-prompt injection.

**D. Recall**
`recall(query)` → embed query → {vec KNN, FTS MATCH, 1-hop graph expansion} → RRF fuse + recency/importance boost → top-k → bump access stats → return.

# Session Notes — 2026-06-10 (M2 + M3 implemented)

State after this session: **M0–M3 complete** (minus two boxes noted below).
Workspace builds with zero warnings; `cargo test --workspace` → 32 passed,
2 ignored (model-download test + figment env test). Smoke-tested CLI
end-to-end: remember → embed → recall (dense path verified: query
"token expiration" matched "expiry" memory) → edges built → status.

## What was built

### M2 — MCP thin slice
- `poneglyph-mcp/src/tools.rs`: `PoneglyphMcp` server, 6 tools via rmcp 1.7
  `#[tool]`/`#[tool_router]`/`#[tool_handler]` macros, structured JSON output
  via `rmcp::Json<T>`. Schemars derives on all request/response types.
- `poneglyph-mcp/src/server.rs`: `run_stdio()` bootstrap (serve → waiting).
- `poneglyph serve` runs the MCP stdio server. **Logging goes to stderr**
  (stdout is JSON-RPC — never println in serve path).
- `core::project`: minimal path-based project detection + ranked context
  builder (importance × recency × access; budget ≈ max_tokens × 4 chars).
  Git-remote normalization still M6.
- `core::retrieve::recall` signature changed: `query_vec: Option<&[f32]>` —
  `None` skips dense path (graceful no-model degradation). FTS queries are
  sanitized (tokens quoted, OR-joined) so natural language can't break
  FTS5 MATCH syntax.
- CLI `remember`/`recall` now actually embed (M1 had a zero-vector
  placeholder). Embedder failure degrades to FTS-only with a warning.
- Integration test `crates/poneglyph-mcp/tests/mcp_roundtrip.rs`: in-process
  rmcp client (duplex transport), asserts DB side effects via a second
  connection. Offline (embedder = None).

### M3 — Graph (no-LLM edges)
- `core::graph`: builders for explicit / similarity (cosine ≥
  `graph.similarity_threshold`, default 0.82, over top-20 KNN candidates) /
  temporal (same project within `graph.temporal_window_secs`, default 300;
  project-less memories skipped) / tag-overlap (≥1 shared tag, weight =
  Jaccard). Symmetric edges stored canonically (min id, max id) +
  INSERT OR IGNORE ⇒ recompute never duplicates (§8.4 AC2).
- `core::model`: new `JobType::ComputeEdges` ("compute_edges").
- `core::enrich`: persistent jobs table is source of truth; tokio mpsc is
  wake-up only. `spawn_worker(db_path, graph_cfg)` opens its own WAL
  connection, drains ≤64 jobs/pass, polls every 30s as fallback. Job
  failures → status `failed`, never crash. LLM job types are marked failed
  ("not implemented until M6") instead of looping.
- Wiring: MCP `remember`/`update_memory` enqueue job + notify (never compute
  inline). `serve` spawns the worker. CLI `remember` enqueues then drains
  inline (one-shot process, no resident worker).

## Decisions / gotchas
- **Embedder optional everywhere** (`Option<Arc<Embedder>>`): offline/CI runs
  FTS-only. MCP integration test relies on this.
- Store is `Arc<std::sync::Mutex<Store>>` in the MCP server; embeddings are
  computed **before** taking the lock (no await under mutex).
- rmcp 1.7: no `tool_router` field needed (MinimalServer pattern);
  `ServerInfo` is non_exhaustive — build via `Default` + field assignment.
- `llm_assist` param is accepted but a no-op until M6.
- Repo: git was initialized **inside** `poneglyph/` this session (parent
  `~/Dev` is an accidental commitless repo). Branch `main`.
  Commits: baseline → M2 → M3.

## Not done / next (M4)
- M2 manual verify: register in Claude Code and round-trip
  (`claude mcp add poneglyph -- /path/to/poneglyph serve`), + Claude Desktop.
- M0 leftover: CI workflow.
- M4: axum HTTP API + /ingest + hooks (poneglyph-http is still stubs);
  `serve` must run MCP + HTTP concurrently (worker handle already exists —
  pass `EnrichHandle` to ingest too).

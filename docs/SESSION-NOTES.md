# Session Notes — 2026-06-10 (Session timeline, M7 hooks/hardening/docs, animated graph)

State after this session: **M7 partially complete** (hooks, failure tests,
migration docs, INSTALL/INTEGRATIONS/README done; remaining: perf bench at 100k,
retrieval eval harness, cross-platform binaries). `cargo test --workspace` → 91
passed, 2 ignored. Viewer builds clean (`pnpm build`).

Commits: `ae6b775` (session timeline + demo seed-only + animated graph) →
`9a03312` (Claude Code hooks) → `1d3564b` (OpenCode plugin) → `4ee9e10`
(failure injection tests) → `9f4c21b` (docs) → `d7d5541` (TODO update).

## What was built

### Session-based memory timeline (Part A)
- `metadata.session_id` hoisted to top-level in `ingest.rs` (canonical key);
  legacy `extra.session_id` handled by SQL `COALESCE(json_extract(...))`.
- `store::list_sessions()` — SQL fetch with COALESCE session_key, Rust-side
  grouping (keyed by session_id, unkeyed by gap-split per project), sorted
  `started_at DESC`, paginated at session level.
- `GET /api/timeline?project_path=&limit=&offset=&gap_secs=` — returns
  sessions with `session_id`, `project_name`, `started_at`, `ended_at`,
  `memory_count`, full `memories` array.
- Demo seeds `demo-session-N` metadata (N = `i/3 + 1`).
- Frontend: `/timeline` route with reui timeline component (vendored from
  reui.io), project filter, load-more pagination.
- 6 new tests: list_sessions (session_id grouping, legacy extra.session_id,
  gap-split, project filter, ordering), ingest session_id hoist, demo
  session_id carry.

### `poneglyph demo` seed-only (Part B)
- Dropped `--port`, added `--force`. Default target is `config.db_path`
  (real store, no ephemeral tempfile). Guard: refuse non-empty DB without
  `--force` or `--db`. Prints hint to run `poneglyph serve`.
- Removed `tempfile` dep from poneglyph-cli.

### Animated graph explorer (Part C)
- Live d3-force simulation in `simRef` (never recreated per render). Stable
  `SimNode` objects in `simNodesRef` (d3 mutates them directly).
- `sim.on('tick')` → rAF-throttled `setNodes` updating only positions.
- Forces: `forceX/forceY` (not center), `forceCollide(85)`, `forceLink`
  distance 180, `forceManyBody` -400. `alphaDecay(0.03)`, `velocityDecay(0.4)`.
- Drag: `fx/fy` on drag start, `alphaTarget(0.3).restart()`, clear on stop.
- `FloatingEdge` component: `useInternalNode` + `getBezierPath` + `BaseEdge`
  + `EdgeLabelRenderer` for relation labels at bezier midpoint.
- All existing features intact (expand-on-click, edge filters, MiniMap,
  selected-memory card, legend, themes).

### Claude Code hooks — perfect
- `posttooluse.sh`: removed `Task` from skip list, added `tool_output`
  (truncated 2000 chars) to payload.
- **New `stop.sh`**: reads `transcript_path` from Stop event stdin, extracts
  last assistant message (reverse JSONL scan), POSTs as `assistant_message`.
- `settings.json.example`: expanded PostToolUse matcher to include `Agent|Task`,
  added Stop hook entry.
- README: documented all 4 hooks, capture matrix, env vars, verify steps.

### OpenCode plugin — perfect
- Added `message.updated` hook for assistant message capture.
- Fixed `directory` to use `ctx.directory` from plugin context.
- Extracted shared `ingest()` helper with `AbortSignal.timeout(2000)`.
- Added `todoread` to skip list.
- README: corrected install path (`.opencode/plugins/` not `.opencode/plugin/`),
  documented all captured events.

### Failure injection tests (M7)
- **store.rs** (11 new tests): double_delete, nonexistent memory ops
  (get/update/merge_metadata), empty DB (list/stats), corrupt metadata JSON,
  importance clamping, delete cascade edges, graph_sample/neighborhood on
  empty/unknown.
- **enrich_llm_test.rs** (3 new tests): corrupt job with missing memory_id
  (FK violation simulation), LLM disabled → fail immediately, edge-only jobs
  with no LLM client.
- Total: 91 tests passing.

### Docs (M7)
- `README.md`: project overview, quick start, architecture diagram, config table.
- `docs/INSTALL.md`: build from source, first run, model download, config, CLI.
- `docs/INTEGRATIONS.md`: Claude Code hooks (all 4), Claude Desktop MCP,
  OpenCode plugin, env vars, security notes.
- `docs/MIGRATION.md`: v0→v1 genesis path, schema version tracking, embedding
  dimension warnings.

## Gotchas
- RTK output for `cargo check` only shows warning locations (file:line) without
  the actual message — use `cargo check` directly if compilation errors need
  debugging.
- Claude Code Stop event provides `transcript_path` (JSONL file). Extracting
  the last assistant message requires reverse-scanning; `tail -r` is macOS
  specific (GNU: `tac`).
- OpenCode plugin events don't have consistent input schemas —
  `message.updated` input has `role`/`content` but typing is loose (`any`).
- The `tail -r` approach for stop.sh works on macOS but would need `tac` on
  Linux. Could use `awk` for portability if needed.

## Not done / next
- M0: CI workflow.
- M2 manual verify: Claude Code + Desktop MCP round-trip.
- M7: perf bench at 100k (criterion), retrieval eval (recall@10), cross-platform
  release binaries.
- Live LLM check vs Ollama (mock-tested only).

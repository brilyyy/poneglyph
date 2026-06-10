# Session Notes — 2026-06-10 (M4 + M5 implemented)

State after this session: **M0–M5 complete** (minus M0 CI workflow and the
M2 manual Claude Code/Desktop round-trip verify). `cargo test --workspace` →
49 passed, 2 ignored; +3 viewer asset tests behind `--features embed-viewer`
(18 total in poneglyph-http). Smoke-tested end-to-end: hook script →
`/ingest` → passive memory tagged tool+project → searchable; embedded viewer
serves with SPA fallback; `/api/graph` returns a 500-node sample + expansion
against a 520-memory seeded DB.

Commits this session: `2808c89` (M4) → `be93d6b` (M5) → `f0169dd` (init fix).

## What was built

### M4 — HTTP + ingest + hooks
- `poneglyph-http` fully implemented: `state.rs` (AppState mirrors the MCP
  pattern — `Arc<Mutex<Store>>`, embed before lock, notify after unlock),
  `error.rs` (`ApiError` → `{"error": msg}` + `ApiJson` extractor so even
  serde rejections keep that shape), `auth.rs`, `api.rs`, `ingest.rs`,
  `viewer.rs`, router in `lib.rs`.
- Routes: `/api/{memories,search,graph,projects,stats,settings}`, `/ingest`,
  `/healthz` (open), `/` fallback (open). axum 0.8 — `{id}` param syntax.
- `Store::graph_neighborhood` (iterative BFS, undirected, node cap, drops
  boundary edges) + `Store::graph_sample` (recent N + edges with both
  endpoints in sample) in core, with unit tests. IN-lists chunked at 900.
- Ingest mapping: tool_use/file_edit/terminal → `code_context`,
  user/assistant_message → `episodic`; source=passive, importance=0.3;
  metadata `{tags: [client, tool?], event, client, tool?, timestamp, extra}`.
  Empty content 400, >100KB 413. No server-side dedupe (hook's job).
- Security (§12, stricter than spec): token enforced whenever set, not just
  non-loopback; `validate_security` refuses non-loopback bind without token;
  GET /api/settings strips secrets (adds `api_token_set`/`api_key_set`);
  PATCH /api/settings whitelist-merges into config.toml and returns
  `restart_required: true` (no hot reload — runtime `Arc<Config>` untouched).
- `cmd_serve`: binds HTTP **before** the select; `AddrInUse` + mcp=true →
  warn + MCP-only (second editor instance survives); `server.mcp=false` →
  HTTP-only daemon until Ctrl-C. MCP stdin close kills the whole process.
- `hooks/claude-code/`: posttooluse.sh + userpromptsubmit.sh (jq transform,
  curl -m 2, always exit 0, skip read-only tools, 4000-char truncate) +
  settings.json.example + README. `hooks/opencode/poneglyph.ts` plugin
  (`tool.execute.after`, swallowed errors — API names unverified, SHOULD).

### M5 — Viewer
- `viewer/`: **TanStack Router + Vite SPA, not TanStack Start** (deviation
  from PRD §13 letter: Start is SSR-first, dead weight under rust-embed).
  Tailwind v4, react-query, hand-rolled UI primitives in
  `src/components/ui.tsx` (shadcn CLI refused non-interactive init; not
  worth fighting). Scaffolded with create-tsrouter-app file-router template.
- Pages: Dashboard, Memories (filters+pagination, URL search params),
  Memory detail (edit/delete, metadata, edges), Search, Graph explorer,
  Settings (whitelisted fields + restart banner).
- Graph explorer: `@xyflow/react` + **static d3-force layout** (300 sync
  ticks, positions reused across expansions so layout stays incremental);
  click node → fetch depth-1 → Map-merge; edge-type checkboxes; node colors
  by memory_type (`TYPE_COLORS` in lib/types.ts).
- Embedding: cargo feature `embed-viewer` on poneglyph-http (CLI re-exports
  as `viewer`), **default off** — plain `cargo build` serves a placeholder
  page, zero Node required. `scripts/build-release.sh` = pnpm build + cargo
  `--features viewer`. SPA fallback: extension-less paths → index.html;
  hashed `/assets/*` get immutable cache headers.

## Decisions / gotchas
- Dev loop: `poneglyph serve` (mcp=false) + `pnpm -C viewer dev` (vite
  proxies /api + /ingest to 3742).
- A release built without `--features viewer` silently ships the placeholder
  — build-release.sh is the only documented release path (M7 checklist item).
- `viewer/src/routeTree.gen.ts` is generated but committed (template default).
- Fixed M0-era bug: `poneglyph init` ENOENT'd when the platform config dir
  didn't exist (now create_dir_all's it).
- Stale-binary trap: first smoke test ran a pre-M4 `target/debug/poneglyph`;
  `cargo test` alone doesn't rebuild the bin you run manually.

## Not done / next (M6)
- M2 manual verify: `claude mcp add poneglyph -- poneglyph serve` round-trip
  + Claude Desktop; now also verify hooks end-to-end in a real session.
- M0 leftover: CI workflow (build + test both feature configs; release job
  runs build-release.sh).
- M6: core::enrich LLM client (async-openai), 4 job types, retry/backoff,
  git-remote project identity, get_project_context token budget already
  exists (M2) — needs git-remote normalization.
- opencode plugin untested against a real opencode install.

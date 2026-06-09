# poneglyph — Implementation TODO

Phase-based. Exit criteria from PRD §14. Auto-updated by Claude Code task hooks.

Legend: `[ ]` pending · `[>]` in progress · `[x]` done

---

## Phase M0 — Skeleton

> Exit: `poneglyph init` creates db + config; CI builds macOS + Linux.

- [x] Workspace Cargo.toml with shared workspace.dependencies
- [x] 4 crates scaffolded (poneglyph-{core,mcp,http,cli}) + module stubs
- [x] .gitignore
- [x] Delete stale top-level src/main.rs
- [>] Implement core::model
- [x] Implement core::config
- [x] Implement core::store + migration runner
- [x] Implement `poneglyph init` CLI command
- [ ] Add CI workflow

---

## Phase M1 — Core + CLI

> Exit: `remember`/`recall`/`forget` work via CLI; AC §8.1–8.3 pass.

- [x] Implement core::embed (embed_anything candle, BAAI/bge-small-en-v1.5, 384d, progress on first-run download)
- [x] Implement core::store CRUD (create/read/update/delete memories; cascade vec+fts+edges on delete)
- [x] vec + fts indexing on create/update; re-embed on content update
- [x] Implement core::retrieve (dense KNN + sparse FTS5 + 1-hop graph; RRF fusion + recency/importance boost)
- [x] Wire CLI: remember, recall, forget, export, status (replace todo!() stubs)
- [x] Unit tests: store CRUD, RRF fusion, delete cascade row-counts

---

## Phase M2 — MCP thin slice

> Exit: Claude Code round-trip store+recall works (§8.6 AC1–2).

- [x] Implement mcp::server (rmcp stdio bootstrap)
- [x] Implement mcp::tools (6 tools via #[tool]: remember recall forget update_memory get_project_context list_memories)
- [x] poneglyph serve starts MCP stdio server
- [x] Integration test: rmcp in-process client; assert DB side effects
- [ ] Manual verify: Claude Code + Claude Desktop MCP round-trip

---

## Phase M3 — Graph (no-LLM edges)

> Exit: §8.4 AC pass; edges visible via API.

- [x] core::enrich queue (tokio mpsc background worker, jobs table groundwork)
- [x] core::graph no-LLM builders: explicit, similarity (cosine ≥ 0.82), temporal (5min window), tag-overlap
- [x] Edge computation enqueued on remember; never blocks
- [x] Unique edge constraint respected on recompute (§8.4 AC2)
- [x] Unit tests: deterministic edge builders given fixtures

---

## Phase M4 — HTTP + ingest + hooks

> Exit: passive capture from Claude Code hook works (§8.7, §8.9).

- [ ] http::api axum router (/api/memories CRUD+filter, /api/search, /api/graph, /api/projects, /api/settings, /api/stats)
- [ ] http::ingest POST /ingest (event schema §10.2 → passive code_context memory + enqueue edges)
- [ ] Bind 127.0.0.1 default; token gate when non-loopback (refuse start without token)
- [ ] poneglyph serve runs MCP + HTTP concurrently
- [ ] hooks/claude-code/ PostToolUse + optional UserPromptSubmit curl-to-/ingest scripts
- [ ] hooks/opencode/ plugin (SHOULD, non-blocking)
- [ ] Integration test: hook POST → stored memory tagged tool+project

---

## Phase M5 — Viewer

> Exit: viewer loads with live data; graph explorer works (§8.12).

- [ ] viewer/ TanStack Start + React + shadcn/ui + React Flow
- [ ] Pages: Dashboard, Memories list+filters, Memory detail, Search, Graph explorer, Settings
- [ ] Build to static assets; embed via rust-embed; served by poneglyph-http
- [ ] Verify: poneglyph serve → localhost loads dashboard; graph renders 500 nodes + expansion

---

## Phase M6 — Enrichment + project context

> Exit: enrichment on/off per §8.11; context injection per §8.10.

- [ ] core::enrich LLM client (async-openai, configurable endpoint, off by default)
- [ ] Job types: summarize, extract_entities, extract_relations, score_importance
- [ ] Retry with backoff → mark failed, never crash/block
- [ ] core::project detect by abs path; normalize git remote for stable identity across clones
- [ ] get_project_context: rank by importance×recency×access, truncate to token budget, return injection string

---

## Phase M7 — Hardening / release

> Exit: PRD §15 release criteria met.

- [ ] Perf pass: seed 100k synthetic memories; bench §11 targets (criterion); CI regression guard
- [ ] Retrieval-quality eval harness: labeled corpus → recall@10 in CI
- [ ] Failure injection tests: unreachable LLM, corrupt job rows, missing model cache → graceful
- [ ] Migration v0→v1 path documented + tested
- [ ] Cross-platform release binaries: macOS arm64 + Linux x86_64
- [ ] Docs: INSTALL.md, INTEGRATIONS.md (Claude Code + Desktop), README quickstart
- [ ] Verify fully offline after first-run model download

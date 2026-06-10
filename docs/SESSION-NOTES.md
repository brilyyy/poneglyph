# Session Notes — 2026-06-10 (XDG paths, demo, context injection, M6, shadcn viewer)

State after this session: **M0–M6 complete** (remaining: M0 CI workflow, M2
manual Claude Code/Desktop verify, M7). `cargo test --workspace` → 69 passed,
2 ignored; +3 viewer tests behind `--features embed-viewer`. Viewer builds
clean (`pnpm build` + `tsc --noEmit`).

Commits: `f2ce21d` (XDG paths) → `08141a9` (demo) → `3ecd80a` (/api/context +
SessionStart hook) → `8c93496` (M6 LLM enrichment) → `2f846c0` (M6 git-remote
identity) → `862727a` (shadcn viewer + UX).

## What was built

### XDG default paths
- config `~/.config/poneglyph/config.toml`, db `~/.local/share/poneglyph/`,
  models `~/.cache/poneglyph/models` (PRD §8.14/§6.1). Windows keeps
  ProjectDirs. `xdg_dir_from` is env-injectable for tests.
- **Legacy fallback is read-only**: old macOS location used in place with a
  warn + `mv` hint (also in `poneglyph status`); never auto-moved (WAL
  sidecars). Explicit config paths unaffected. This machine still runs from
  the legacy dir — move it when convenient.

### `poneglyph demo`
- Seeds ~20 templates cycled to `--count` (default 60): 3 fake projects, all
  6 types, tags, 2 near-dup pairs, backdated 30 days in temporal clusters
  (raw SQL, demo-only), inline edge drain, hand-seeded explicit + labeled
  relation edges. **Ephemeral TempDir DB by default**; `--db`/`--port` flags.
  Serves HTTP-only with the embedded viewer.

### Zero-token session context injection
- `GET /api/context?project_path=&max_tokens=` wraps
  `project::get_project_context` (behind bearer auth).
- `hooks/claude-code/sessionstart.sh`: SessionStart stdout → injected
  context. Budget `PONEGLYPH_CONTEXT_TOKENS` (default 600; config 2000 stays
  for the MCP tool). Zero LLM calls; silent exit 0 when server down.

### M6 — enrichment
- `core::llm`: async-openai vs configurable endpoint. `from_config` → None
  unless enabled+endpoint+model; worker constructs it only when
  `enrichment.enabled` (AC1 — disabled ⇒ client never exists).
- Handlers: summarize→metadata.summary (skip <280 chars);
  extract_entities→entities+tags union → re-enqueue compute_edges;
  extract_relations→relation edges grounded to top-5 embedding neighbours,
  LLM picks by index (no dangling nodes); score_importance→importance.
- Worker: plain async task **owning** Store, `&mut Store` through the async
  chain (Store is Send not Sync; `&mut T` is Send — `&T` isn't). No more
  spawn_blocking shuttle.
- Retry: attempts bumped only by `mark_job_running`; failure → pending with
  updated_at as retry stamp; due-filter in Rust (10s·2^attempts); failed at
  3. Sync CLI drain skips LLM jobs (leaves pending for serve worker).
- Wiring: MCP remember `llm_assist` + both config gates → 4 jobs; /ingest →
  summarize only (passive volume). Tests vs in-process axum mock (real wire
  path) incl. garbage-reply retry-to-failed and unreachable-endpoint
  failure injection.

### M6 — git-remote identity
- `read_git_remote` parses `.git/config` directly (follows `gitdir:`
  pointers); `normalize_git_remote` → `host/org/repo` (ssh/scp/https/user@/
  port/.git forms). detect_project: path hit (backfills NULL remote) →
  remote hit (same project, **original path kept**, AC2) → new upsert.
  CLI remember now uses detect_project.

### Viewer — shadcn + UX
- User had run `shadcn init` (style radix-luma, hugeicons, `#/` aliases);
  this session added ~20 components and migrated everything; `ui.tsx`
  deleted; `@/*` alias dropped from tsconfig. TypeBadge wraps shadcn Badge.
- UX: icon-collapsible Sidebar + live stats footer + dark-mode toggle
  (localStorage + pre-paint script in index.html); sonner toasts;
  AlertDialog delete; Table memories list, relative timestamps
  (`formatRelative`); debounced search-as-you-type (`keepPreviousData`);
  graph MiniMap + colorMode + selected-node card; Field/Switch settings;
  dashboard type-breakdown bars. `/api/stats` gained `by_type` (GROUP BY).

## Gotchas
- Generated shadcn `spinner.tsx` conflicts with HugeiconsIcon strokeWidth
  typing — fixed with `Omit<ComponentProps<"svg">, "strokeWidth">`. May
  recur if the component is re-generated with `--overwrite`.
- `XDG paths commit (f2ce21d) accidentally also picked up the user's
  untracked shadcn init files — intentional content, early landing.
- hugeicons names verified against dist/types/index.d.ts (5473 icons);
  NeuralNetworkIcon = graph nav icon.
- Demo with model available takes ~20 s to embed 30+ seeds before the
  server binds — don't curl too early.

## Not done / next
- M0: CI workflow (test default + `--features embed-viewer`; release job
  runs scripts/build-release.sh).
- M2 manual verify: Claude Code + Desktop MCP round-trip; hooks end-to-end
  in a real session (incl. new sessionstart.sh).
- Live LLM check vs Ollama (mock-tested only).
- M7 hardening: perf bench at 100k, retrieval eval, migration docs,
  release binaries, INSTALL/INTEGRATIONS docs.

# Changelog

Notable changes on `refactor/unified-v2`. Earlier history: `git log`.

## v1.0.0 — Claude Code focus, MCP hooks, error handling

- **Claude Code only by default.** Other agent integrations (Cursor, Gemini,
  OpenCode, Codex, Copilot) are feature-gated behind `--features cursor`,
  `gemini`, `opencode`, `codex`, `copilot`, or `all-agents`. They compile
  out entirely in the default build. This cuts compile time and binary size
  for the 90% case.
- **Hooks switched from HTTP to CLI.** Claude Code hooks now call
  `poneglyph remember` / `poneglyph context` directly instead of curling
  `/ingest`. No HTTP server needed for passive capture — just `poneglyph`
  on PATH. The `PONEGLYPH_PORT` and `PONEGLYPH_TOKEN` env vars are no
  longer used by hooks.
- **New `poneglyph context` CLI subcommand.** Returns ranked project memory
  for session injection, same as the `/api/context` HTTP endpoint but
  callable from hooks without a running server.
- **`poneglyph-http` is now optional.** The `viewer` feature (default off
  for `cargo install`) gates the HTTP dashboard. `poneglyph viewer` prints
  a clear message when the feature is absent.
- **Error handling overhaul.** CLI errors are categorized (user/environment/
  internal) with distinct exit codes (1 for user/env, 2 for internal).
  Internal errors show full cause chains only with `RUST_LOG=debug`.
  MCP errors use `invalid_params` for user mistakes vs `internal_error`
  for bugs.
- **Agent defaults changed.** Only `claude_code` is enabled by default in
  `[agents]` config. Other agents default to `false`.
- **Hooks no longer need jq for payload construction.** Simplified scripts
  that call `poneglyph remember` directly.

## Phase G — Model picker, opt-in LLM provider features, serve/viewer split

- `poneglyph init` interactively offers 3 curated 384d embedding models
  (multilingual MiniLM, English `bge-small`, English `all-MiniLM-L6-v2`)
  with pros/cons, instead of always defaulting to one. Falls straight
  through to the first model on non-TTY stdin (CI/scripts stay
  script-safe). A 384d-vs-768d build-time tier was scoped out for now — the
  `vec_memories` sqlite-vec width can't change after the fact, so doing
  that properly needs a Cargo feature, a `format!`-built schema DDL, and a
  stored-dimension compatibility check; not worth it for a second tier
  nobody's asked for yet. (Researched in passing: `embed_anything` already
  has an `ort` ONNX-runtime feature that loads any HF repo's `model.onnx`
  generically, unlike the candle backend's fixed architecture-string match
  — the path to true e5/BGE-M3-class multilingual models later, if wanted,
  without replacing `embed_anything`.)
- LLM-backed enrichment/compression is now opt-in per provider —
  `llm-openai`/`llm-anthropic`/`llm-gemini`/`llm-all` Cargo features, all
  off by default (was: all three always compiled in, gated only by runtime
  config). Compression itself has no provider-specific code — it's always
  one `LlmClient::complete` call — so this gates the three `Backend`
  implementations directly; a provider configured without its feature
  compiled in degrades gracefully (caveman fallback, clear warning) rather
  than failing to build.
- `poneglyph serve` is now MCP-only (stdio); the HTTP dashboard + graph
  viewer moved to a new standalone `poneglyph viewer` command. They're
  independent processes against the same database — no more shared-process
  `dashboard.mcp` toggle (removed) or "HTTP port busy, falling back to
  MCP-only" degradation. Passive-capture hooks (`posttooluse.sh` etc.) POST
  to `/ingest`, so they now need `poneglyph viewer` running, not `serve`.

## Phase F — Multilingual embeddings, graph-first guidance, tree-sitter registry

- Default embedding model swapped from `BAAI/bge-small-en-v1.5` (English-only)
  to `sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2` (50+
  languages, still 384d — no schema change). `intfloat/multilingual-e5-base`
  was tried first but its `XLMRobertaModel` architecture isn't supported by
  `embed_anything`'s candle backend (Bert/Jina/ModernBert/Qwen3/Model2Vec
  only); MiniLM is `BertModel`-based and loads cleanly. Cross-lingual recall
  verified (French memory ↔ English query).
- `Embedder::embed_query` / `embed_passage` replace the role-agnostic
  `embed_text` at all store/recall call sites, with configurable
  `[embedding].query_prefix` / `passage_prefix` (empty by default; e5-family
  models need `"query: "` / `"passage: "`).
- MCP server instructions, the injected `CLAUDE.md`/`AGENTS.md`/`.cursorrules`
  rules block, and `SKILL.md` now push agents to try `codegraph_query`
  (including a bare keyword search) before grep/glob for any "find/explore"
  question, not just call/import/test impact analysis — a graph lookup
  doesn't get slower as the repo grows the way a directory walk does.
- `codegraph::parse` consolidated its three parallel per-language `match`
  blocks (extension routing, grammar loading, node-kind tables) into one
  `LANGS` table — adding a language is now one `tree-sitter-<lang>` dep plus
  one table row instead of three match arms kept in sync by hand.
- `poneglyph init` prints a small ASCII rendering of the logo (red, stays off
  `serve`'s stdout, which is reserved for MCP JSON-RPC).

## Phase E — GPU graph viewer, parallel codegraph build, self-healing hooks

- Cargo: `[profile.dev]`/`[profile.release]` (lto, codegen-units, opt-level).
- `codegraph::build` parses files in parallel via rayon (DB writes stay serial).
- New `path:<a>..<b>` codegraph query — shortest call/import chain between
  two symbols (CLI, MCP `codegraph_query`).
- `/api/graph` and `/api/codegraph` now return exact `total_nodes`/
  `total_edges` alongside the sampled arrays; the render cap is configurable
  via `[graph].max_render_nodes` (default 50000, was a hardcoded 2000).
- Viewer: `/graph` and `/codegraph` swapped from React Flow (DOM/SVG) +
  main-thread d3-force to `@cosmos.gl/graph` (WebGL, GPU simulation) —
  scales well past what the old stack could render. Both views show a
  "showing X of Y (sampled)" banner and a render-limit slider; node
  size/opacity now encode importance/tier/connection-count, fields that
  previously had no visual representation. Status page gained a "Graph
  coverage" card and a `/api/context` preview panel.
- `posttooluse.sh` debounce-triggers `poneglyph graph update` after source
  edits (skip if triggered <10s ago), so the code graph self-heals without a
  separate `graph watch` process; `sessionstart.sh` nudges toward
  `graph init` when the graph is empty.
- `poneglyph init --inject-rules` (opt-in): idempotently injects a condensed
  usage block into `CLAUDE.md`/`AGENTS.md`/`.cursorrules`, for any that
  already exist. Never creates new files.
- `SKILL.md` expanded with the `path:` syntax and a full 8-tool reference.

## Phase D — Semantic compression pipeline (`65699a1`)

Schema v4: `memories.compressed_content` / `memories.compression_mode`.
New `[memory].compression_mode` (`caveman` | `semantic`) and
`JobType::ExtractCompress` background job — LLM-extractive rewrite,
caveman-compressed again, cached for context injection only. Falls back
to caveman-only when no LLM is reachable. `content` itself is never
overwritten. See [docs/COMPRESSION.md](docs/COMPRESSION.md).

## Phase C — Codegraph dashboard query optimization (`d92f733`)

Fixed N+1 queries and a full-edge-table scan in the `/api/codegraph`
handler; added `focus`/`depth`/`limit` query params so the dashboard can
center on a blast-radius trace instead of always loading the whole graph.

## Phase B — `.poneglyphignore` support (`1fda7ad`)

Code graph builds now honor a project-root `.poneglyphignore`
(gitignore-style syntax) merged with `[graph].exclude_patterns`.
Deliberately does not read `.gitignore` itself. See
[docs/CODEGRAPH.md](docs/CODEGRAPH.md#poneglyphignore).

## Phase A — Claude Code skill + OpenCode parity (`35ae27c`)

Added `hooks/poneglyph/SKILL.md` teaching Claude Code when to use
`remember`/`recall`/`get_project_context` and `codegraph_query`/
`codegraph_blast_radius` instead of ad-hoc grepping. Documented OpenCode's
plugin-injection limitation (`hooks/opencode/README.md`) — no
return-stdout-becomes-context mechanism exists there yet, so it's a
best-effort debug-log only, not real context injection.

---

Foundational work these phases build on (see `git log` for full detail):
Tree-sitter code knowledge graph engine (`6e01362`), `poneglyph graph` CLI
+ MCP tools (`207a85e`), codegraph/token-savings/agent-status dashboard
APIs and views (`c171c58`, `b740540`).

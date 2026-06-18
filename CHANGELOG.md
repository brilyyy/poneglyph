# Changelog

Notable changes on `refactor/unified-v2`. Earlier history: `git log`.

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

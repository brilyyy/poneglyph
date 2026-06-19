<p align="center">
  <img src="viewer/public/logo.svg" alt="poneglyph" width="120">
</p>

# poneglyph

Local AI memory engine for coding agents. Remembers what your tools do so your
agent has project context across sessions.

## Features

- **Hybrid retrieval** — dense embeddings (384d) + FTS5 + 1-hop graph expansion with RRF fusion
- **Knowledge graph** — explicit, similarity, temporal, tag-overlap, and optional LLM relation edges
- **Code knowledge graph** — Tree-sitter parsed callers/callees/imports/tests across rust/ts/js/python/go, with `.poneglyphignore` support, parallel (rayon) parsing, and a `path:<a>..<b>` shortest-chain query
- **MCP server** — 6 memory tools (remember, recall, forget, update, context, list) + 2 codegraph tools (`codegraph_query`, `codegraph_blast_radius`)
- **Claude Code skill + OpenCode plugin** — teaches agents when to use memory/codegraph tools instead of ad-hoc grepping; `poneglyph init --inject-rules` can also inject a condensed usage block into an existing `CLAUDE.md`/`AGENTS.md`/`.cursorrules`
- **Self-healing code graph** — the PostToolUse hook debounce-triggers `graph update` after source edits, so `codegraph_query`/`codegraph_blast_radius` stay fresh without a separate watch process
- **Passive capture** — Claude Code hooks + OpenCode plugin auto-capture tool executions, prompts, and assistant messages
- **Web viewer** — dashboard, memories list/detail, search, timeline, token-savings, agent status, settings, and a GPU-rendered (WebGL) graph explorer + codegraph view that scales well past what a DOM-based renderer can handle, with a "showing X of Y" sampling indicator and render-limit slider. Runs as its own command (`poneglyph viewer`), independent of the MCP server (`poneglyph serve`)
- **Zero-token context injection** — session context from your project's memories, no LLM calls
- **Multilingual embeddings by default** — `poneglyph init` interactively offers 3 curated 384d models spanning multilingual to English-only, with pros/cons for each
- **Optional LLM enrichment** — summarize, extract entities/relations, score importance, semantic compression (off by default, and not compiled in unless you build with `--features llm-openai`/`llm-anthropic`/`llm-gemini`/`llm-all`)
- **Fully offline** — after first-run model download, everything runs locally

## Quick start

```sh
curl -fsSL https://raw.githubusercontent.com/brilyyy/poneglyph/main/scripts/install.sh | bash
```

Installs a prebuilt binary for your platform (falling back to cloning and
building from source if none is available), then runs `poneglyph init`.

```sh
# Start the MCP server (for your editor/agent)
poneglyph serve

# Start the web dashboard + graph viewer (separate process)
poneglyph viewer
open http://127.0.0.1:3742
```

### Build from source

```sh
git clone https://github.com/brilyyy/poneglyph.git
cd poneglyph
cargo build --release
./target/release/poneglyph init
./target/release/poneglyph serve   # MCP, for editor/agent integration
```

LLM-backed enrichment/compression is opt-in per provider and not compiled in
by default — add `--features llm-openai`, `llm-anthropic`, `llm-gemini`, or
the `llm-all` bundle to `cargo build` if you want it.

## Demo

```sh
# Seed sample data and view in browser
./target/release/poneglyph demo
./target/release/poneglyph viewer
open http://127.0.0.1:3742
```

## Documentation

- [INSTALL.md](docs/INSTALL.md) — build from source, configuration, first run
- [INTEGRATIONS.md](docs/INTEGRATIONS.md) — Claude Code, Claude Desktop, OpenCode setup
- [CODEGRAPH.md](docs/CODEGRAPH.md) — code knowledge graph CLI, `.poneglyphignore`, MCP tools, dashboard
- [COMPRESSION.md](docs/COMPRESSION.md) — semantic compression pipeline
- [MIGRATION.md](docs/MIGRATION.md) — schema migration guide
- [CHANGELOG.md](CHANGELOG.md) — notable changes by phase
- [PRD](docs/poneglyph_PRD.md) — full product requirements document

## Architecture

```
poneglyph-cli       ── clap binary (init, serve, viewer, remember, recall, demo, ...)
poneglyph-http      ── axum server (/ingest, /api/*, embedded viewer)
poneglyph-mcp       ── rmcp stdio server (8 tools: memory + codegraph)
poneglyph-core      ── store, embed, retrieve, graph, codegraph, compress, enrich, llm, config
viewer/             ── TanStack Router + React SPA; graph views render via cosmos.gl (WebGL)
hooks/claude-code/  ── bash hooks (posttooluse, userpromptsubmit, stop, sessionstart)
hooks/opencode/     ── TypeScript plugin
hooks/poneglyph/    ── Claude Code skill (SKILL.md)
```

## Configuration

TOML at `~/.config/poneglyph/config.toml` (XDG). Key settings:

| Setting | Default | Purpose |
|---|---|---|
| `dashboard.port` | `3742` | `poneglyph viewer` HTTP port |
| `embedding.model_id` | `sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2` | Embedding model (384d); picked interactively at `poneglyph init` |
| `llm.enabled` | `false` | Optional LLM enrichment (also needs a matching `--features llm-*` build) |
| `enrichment.enabled` | `false` | Enable enrichment jobs |

## License

MIT

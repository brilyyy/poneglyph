# poneglyph

Local AI memory engine for coding agents. Remembers what your tools do so your
agent has project context across sessions.

## Features

- **Hybrid retrieval** — dense embeddings (384d) + FTS5 + 1-hop graph expansion with RRF fusion
- **Knowledge graph** — explicit, similarity, temporal, tag-overlap, and optional LLM relation edges
- **Code knowledge graph** — Tree-sitter parsed callers/callees/imports/tests across rust/ts/js/python/go, with `.poneglyphignore` support
- **MCP server** — 6 memory tools (remember, recall, forget, update, context, list) + 2 codegraph tools (`codegraph_query`, `codegraph_blast_radius`)
- **Claude Code skill + OpenCode plugin** — teaches agents when to use memory/codegraph tools instead of ad-hoc grepping
- **Passive capture** — Claude Code hooks + OpenCode plugin auto-capture tool executions, prompts, and assistant messages
- **Web viewer** — dashboard, memories list/detail, search, graph explorer, timeline, codegraph, token-savings, agent status, settings
- **Zero-token context injection** — session context from your project's memories, no LLM calls
- **Optional LLM enrichment** — summarize, extract entities/relations, score importance, semantic compression (off by default)
- **Fully offline** — after first-run model download, everything runs locally

## Quick start

```sh
curl -fsSL https://raw.githubusercontent.com/brilyyy/poneglyph/main/scripts/install.sh | bash
```

Installs a prebuilt binary for your platform (falling back to cloning and
building from source if none is available), then runs `poneglyph init`.

```sh
# Start MCP + HTTP server
poneglyph serve

# Open viewer
open http://127.0.0.1:3742
```

### Build from source

```sh
git clone https://github.com/brilyyy/poneglyph.git
cd poneglyph
cargo build --release
./target/release/poneglyph init
./target/release/poneglyph serve
```

## Demo

```sh
# Seed sample data and view in browser
./target/release/poneglyph demo
./target/release/poneglyph serve
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
poneglyph-cli       ── clap binary (init, serve, remember, recall, demo, ...)
poneglyph-http      ── axum server (/ingest, /api/*, embedded viewer)
poneglyph-mcp       ── rmcp stdio server (8 tools: memory + codegraph)
poneglyph-core      ── store, embed, retrieve, graph, codegraph, compress, enrich, llm, config
viewer/             ── TanStack Router + React + React Flow SPA
hooks/claude-code/  ── bash hooks (posttooluse, userpromptsubmit, stop, sessionstart)
hooks/opencode/     ── TypeScript plugin
hooks/poneglyph/    ── Claude Code skill (SKILL.md)
```

## Configuration

TOML at `~/.config/poneglyph/config.toml` (XDG). Key settings:

| Setting | Default | Purpose |
|---|---|---|
| `server.http_port` | `3742` | HTTP API + viewer port |
| `server.mcp` | `true` | Enable MCP stdio server |
| `embedding.model_id` | `BAAI/bge-small-en-v1.5` | Embedding model (384d) |
| `llm.enabled` | `false` | Optional LLM enrichment |
| `enrichment.enabled` | `false` | Enable enrichment jobs |

## License

MIT

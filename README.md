# poneglyph

Local AI memory engine for coding agents. Remembers what your tools do so your
agent has project context across sessions.

## Features

- **Hybrid retrieval** — dense embeddings (384d) + FTS5 + 1-hop graph expansion with RRF fusion
- **Knowledge graph** — explicit, similarity, temporal, tag-overlap, and optional LLM relation edges
- **MCP server** — 6 tools for Claude Code / Claude Desktop (remember, recall, forget, update, context, list)
- **Passive capture** — Claude Code hooks + OpenCode plugin auto-capture tool executions, prompts, and assistant messages
- **Web viewer** — dashboard, memories list/detail, search, graph explorer, timeline, settings
- **Zero-token context injection** — session context from your project's memories, no LLM calls
- **Optional LLM enrichment** — summarize, extract entities/relations, score importance (off by default)
- **Fully offline** — after first-run model download, everything runs locally

## Quick start

```sh
# Build
cargo build --release

# Initialize (creates db + config)
./target/release/poneglyph init

# Start MCP + HTTP server
./target/release/poneglyph serve

# Open viewer
open http://127.0.0.1:3742
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
- [MIGRATION.md](docs/MIGRATION.md) — schema migration guide
- [PRD](docs/poneglyph_PRD.md) — full product requirements document

## Architecture

```
poneglyph-cli       ── clap binary (init, serve, remember, recall, demo, ...)
poneglyph-http      ── axum server (/ingest, /api/*, embedded viewer)
poneglyph-mcp       ── rmcp stdio server (6 tools)
poneglyph-core      ── store, embed, retrieve, graph, enrich, llm, config
viewer/             ── TanStack Router + React + React Flow SPA
hooks/claude-code/  ── bash hooks (posttooluse, userpromptsubmit, stop, sessionstart)
hooks/opencode/     ── TypeScript plugin
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

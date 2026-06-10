# Installation Guide

## Prerequisites

- **Rust 1.75+** (edition 2024) — install via [rustup](https://rustup.rs)
- **jq** and **curl** — required for Claude Code hooks (macOS: `brew install jq`)

## Build from source

```sh
git clone https://github.com/your-org/poneglyph.git
cd poneglyph
cargo build --release
```

Binary: `target/release/poneglyph`

## First run

```sh
# Create database + default config
poneglyph init

# Start MCP + HTTP server
poneglyph serve
```

`poneglyph init` creates:
- Config: `~/.config/poneglyph/config.toml`
- Database: `~/.local/share/poneglyph/poneglyph.db`
- Model cache: `~/.cache/poneglyph/models/`

## Model download

On first `recall`, `remember`, or `serve`, the embedding model
(`BAAI/bge-small-en-v1.5`, ~30MB) downloads to `~/.cache/poneglyph/models/`.
After this, everything runs **fully offline**.

## Verify

```sh
# Check server health
curl http://127.0.0.1:3742/healthz

# Open viewer
open http://127.0.0.1:3742

# Store and recall a memory
poneglyph remember "Postgres connection pool capped at 20 in production"
poneglyph recall "postgres pool" --limit 3
```

## Configuration

Edit `~/.config/poneglyph/config.toml`:

```toml
[server]
http_port = 3742
bind_addr = "127.0.0.1"
mcp = true

[embedding]
model_id = "BAAI/bge-small-en-v1.5"
dimensions = 384

[llm]
enabled = false
# endpoint = "http://localhost:11434/v1"
# model = "llama3"

[enrichment]
enabled = false
```

Environment variables override config (prefix `PONEGLYPH_`):
- `PONEGLYPH_PORT` — HTTP port
- `PONEGLYPH_TOKEN` — API bearer token (required if non-loopback bind)
- `PONEGLYPH_CONTEXT_TOKENS` — SessionStart context budget (default 600)

## CLI commands

| Command | Purpose |
|---|---|
| `poneglyph init` | Create db + default config |
| `poneglyph serve` | Start MCP + HTTP servers |
| `poneglyph remember "<text>"` | Store a memory |
| `poneglyph recall "<query>"` | Search memories |
| `poneglyph forget <id>` | Delete a memory |
| `poneglyph export --format json` | Export all memories |
| `poneglyph status` | Show db path, counts, model info |
| `poneglyph demo` | Seed sample data |

## Next steps

- [INTEGRATIONS.md](INTEGRATIONS.md) — set up Claude Code hooks, Claude Desktop MCP, or OpenCode plugin
- [MIGRATION.md](MIGRATION.md) — schema migration guide

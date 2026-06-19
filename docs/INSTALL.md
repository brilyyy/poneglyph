# Installation Guide

## Prerequisites

- **Rust 1.75+** (edition 2024) — install via [rustup](https://rustup.rs)
- **jq** and **curl** — required for Claude Code hooks (macOS: `brew install jq`)

## Install via npm

```sh
npm install -g poneglyph
```

Downloads a prebuilt binary for your platform (macOS arm64/x86_64, Linux
x86_64, Windows) from GitHub Releases — no Rust toolchain required. Falls
back to printing a `cargo install` hint if no prebuilt binary matches your
platform or the release is missing.

## Install via cargo

```sh
cargo install poneglyph
```

## Build from source

```sh
git clone https://github.com/brilyyy/poneglyph.git
cd poneglyph
cargo build --release
```

Binary: `target/release/poneglyph`

LLM-backed enrichment/compression is opt-in per provider and not compiled in
by default (keeps the default binary smaller and dependency-free of
`async-openai`/provider SDKs). Add features to pull a provider in:

```sh
cargo build --release --features llm-anthropic   # or llm-openai, llm-gemini, llm-all
```

## First run

```sh
# Create database + default config (interactively picks an embedding model)
poneglyph init

# Start the MCP server (for your editor/agent)
poneglyph serve

# Start the web dashboard + graph viewer (separate process)
poneglyph viewer
```

`poneglyph init` creates:
- Config: `~/.config/poneglyph/config.toml`
- Database: `~/.local/share/poneglyph/poneglyph.db`
- Model cache: `~/.cache/poneglyph/models/`

## Model download

On first `recall`, `remember`, `serve`, or `viewer`, the embedding model
picked at `poneglyph init` (default
`sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2`) downloads to
`~/.cache/poneglyph/models/`. After this, everything runs **fully offline**.

## Verify

```sh
# poneglyph viewer must be running for these two
curl http://127.0.0.1:3742/healthz
open http://127.0.0.1:3742

# Store and recall a memory (works standalone, no server needed)
poneglyph remember "Postgres connection pool capped at 20 in production"
poneglyph recall "postgres pool" --limit 3
```

## Configuration

Edit `~/.config/poneglyph/config.toml`:

```toml
[dashboard]
port = 3742
host = "127.0.0.1"

[embedding]
model_id = "sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2"
dimensions = 384

[llm]
enabled = false
# endpoint = "http://localhost:11434/v1"
# model = "llama3"

[enrichment]
enabled = false

[memory]
compression_enabled = false
compression_mode = "caveman"  # "caveman" | "semantic"

[graph]
enabled = true
exclude_patterns = ["**/target/**", "**/node_modules/**", "**/.git/**", "**/*.test.ts", "**/*_test.rs"]
blast_radius_depth = 5
```

`[llm]` (and therefore `compression_mode = "semantic"`) needs a binary built
with a matching provider feature — see [Build from source](#build-from-source)
above. With none compiled in, semantic compression degrades to the caveman
fallback automatically; it never blocks `remember`.

See [COMPRESSION.md](COMPRESSION.md) for `[memory]` compression detail and
[CODEGRAPH.md](CODEGRAPH.md) for `[graph]` and `.poneglyphignore`.

Environment variables override config (prefix `PONEGLYPH_`):
- `PONEGLYPH_PORT` — HTTP port (`poneglyph viewer`)
- `PONEGLYPH_TOKEN` — API bearer token (required if non-loopback bind)
- `PONEGLYPH_CONTEXT_TOKENS` — SessionStart context budget (default 600)

## CLI commands

| Command | Purpose |
|---|---|
| `poneglyph init` | Create db + default config |
| `poneglyph serve` | Start the MCP server (stdio) — editor/agent integration |
| `poneglyph viewer` | Start the web dashboard + graph viewer (HTTP), separate process |
| `poneglyph remember "<text>"` | Store a memory |
| `poneglyph recall "<query>"` | Search memories |
| `poneglyph forget <id>` | Delete a memory |
| `poneglyph export --format json` | Export all memories |
| `poneglyph status` | Show db path, counts, model info |
| `poneglyph demo` | Seed sample data |
| `poneglyph graph init [path]` | Full code-graph build (Tree-sitter parse) under `path` (default `.`) |
| `poneglyph graph update [path]` | Incremental code-graph rebuild (changed files only) |
| `poneglyph graph watch [path]` | Watch and incrementally rebuild on change |
| `poneglyph graph query "<q>"` | `callers_of:`/`callees_of:`/`imports_of:`/`tests_for:`/keyword query |
| `poneglyph graph blast-radius <target> [--depth N]` | Recursive caller/importer/test trace |
| `poneglyph graph export --format json\|dot\|graphml [--out path]` | Export the code graph |

See [CODEGRAPH.md](CODEGRAPH.md) for full detail on the code graph,
`.poneglyphignore`, and the matching MCP tools.

## Next steps

- [INTEGRATIONS.md](INTEGRATIONS.md) — set up Claude Code hooks, Claude Desktop MCP, or OpenCode plugin
- [CODEGRAPH.md](CODEGRAPH.md) — code knowledge graph, `.poneglyphignore`
- [COMPRESSION.md](COMPRESSION.md) — semantic compression pipeline
- [MIGRATION.md](MIGRATION.md) — schema migration guide

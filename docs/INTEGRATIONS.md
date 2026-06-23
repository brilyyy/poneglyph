# Integrations

poneglyph integrates with Claude Code through hooks, MCP tools, and a skill.

## Claude Code

### Quick setup

```sh
poneglyph wire claude-code
```

This configures:
- MCP server registered as `http://127.0.0.1:27271/mcp` in `~/.claude.json`
  — keep `poneglyph mcp` running (it's a persistent daemon, not
  session-spawned)
- Hooks in `~/.config/poneglyph/hooks/` and `~/.claude/settings.json`
- Skill in `~/.claude/skills/poneglyph/SKILL.md`

### Hooks

Hooks auto-capture tool executions, user prompts, and assistant messages as
passive memories. Session context is injected on new sessions.

Hooks call `poneglyph` CLI directly — no HTTP server needed, just
`poneglyph` on PATH.

```sh
# Automatic (recommended):
poneglyph wire claude-code

# Manual:
mkdir -p ~/.config/poneglyph/hooks
cp hooks/claude-code/*.sh ~/.config/poneglyph/hooks/
chmod +x ~/.config/poneglyph/hooks/*.sh
```

### What gets captured

| Hook | Event | Memory type | Content |
|---|---|---|---|
| `posttooluse.sh` | PostToolUse | `code_context` | Tool name + input + output. Also debounce-triggers `poneglyph graph update` after source edits. |
| `userpromptsubmit.sh` | UserPromptSubmit | `episodic` | User prompt text |
| `stop.sh` | Stop | `episodic` | Last assistant message. Also debounce-triggers `poneglyph consolidate` (~30 min), promoting raw captures into episodic/semantic/procedural tiers. |
| `sessionstart.sh` | SessionStart | *(read-only)* | Injects project context via `poneglyph context` |

### Consolidation pipeline

Raw captures aren't the end of the story — a background pipeline promotes
them through tiers: `episodic` session summaries → cross-session `semantic`
facts (with confidence + lineage) → `procedural` workflows. This runs in two
places: the `poneglyph mcp` daemon's scheduler (`[consolidation]
interval_hours`, default every 6h), and the Stop hook's debounced trigger
above — so the deeper tiers still form even without the daemon running. Every
stage has a deterministic fallback (embedding-cluster summaries, frequent
tool-sequence mining) when `[llm] enabled = false`, so this works identically
with or without a local LLM. See [HOW_IT_WORKS.md](HOW_IT_WORKS.md) for the
LLM-path vs. fallback details per stage.

### Session context injection

On every new session, `sessionstart.sh` calls `poneglyph context` to load
your project's most relevant memories (ranked by importance × recency ×
access). Zero LLM calls — computed entirely locally.

Control the budget with `PONEGLYPH_CONTEXT_TOKENS` (default 600).

### Skill

Teaches Claude Code when to reach for `remember`/`recall`/`get_project_context`
and `codegraph_query`/`codegraph_blast_radius` instead of ad-hoc grepping:

```sh
# Installed automatically by `poneglyph wire claude-code`
# Manual: ~/.claude/skills/poneglyph/SKILL.md
```

### Inject usage rules into CLAUDE.md / AGENTS.md / .cursorrules

```sh
poneglyph wire claude-code  # auto-injects into global CLAUDE.md
```

For project-level injection, idempotently inserts/replaces a fenced
`<!-- poneglyph:start --> ... <!-- poneglyph:end -->` block into files
that already exist. Never creates a file that doesn't already exist.

### Verify

```sh
# Use Claude Code in a project — tool calls are captured automatically

# Check captured memories
poneglyph recall "recent work" --limit 5
```

## Claude Desktop (MCP)

`poneglyph mcp` runs as a persistent HTTP daemon on `127.0.0.1:27271` by
default (configurable via `agents.mcp_server_port`) — start it once
(`poneglyph mcp &`) and point clients at it by URL:

```json
{
  "mcpServers": {
    "poneglyph": {
      "type": "http",
      "url": "http://127.0.0.1:27271/mcp"
    }
  }
}
```

Clients that only support spawning a stdio command (no remote MCP) can use
the legacy escape hatch instead — `poneglyph mcp --stdio` skips the HTTP
server and speaks JSON-RPC over stdio per-process, same as before 1.1.0:

```json
{
  "mcpServers": {
    "poneglyph": {
      "command": "/path/to/poneglyph",
      "args": ["mcp", "--stdio"]
    }
  }
}
```

### Available MCP tools

| Tool | Description |
|---|---|
| `remember` | Store a memory with type, importance, project, tags |
| `recall` | Search memories (hybrid dense + sparse + graph) |
| `forget` | Delete a memory by ID |
| `update_memory` | Edit content, importance, or metadata |
| `get_project_context` | Get ranked context string for a project |
| `list_memories` | List memories with filters |
| `codegraph_query` | Query the code knowledge graph (`callers_of:`/`callees_of:`/`imports_of:`/`tests_for:`/`path:<a>..<b>`/keyword) |
| `codegraph_blast_radius` | Recursive caller/importer/test trace — what breaks if this changes |

Both codegraph tools require `poneglyph graph init` to have been run first
— see [CODEGRAPH.md](CODEGRAPH.md).

## Local LLM enrichment (Ollama)

Session summaries, semantic fact distillation, procedural workflow synthesis,
entity/relation extraction, and importance scoring can all run through a
local model instead of staying purely extractive/deterministic. Reached over
the OpenAI-compatible HTTP backend; Ollama is the default provider, so this
works with a plain `ollama pull` — no API key, nothing leaves your machine.

Enable in `~/.config/poneglyph/config.toml`:

```toml
[llm]
enabled = true
provider = "ollama"        # default; also: lmstudio, gpt4all, openai
model = "qwen2.5:7b-instruct"
# base_url defaults to http://localhost:11434/v1 for ollama

[enrichment]
enabled = true
```

Recall also needs the binary built with `--features llm-openai` (in the
default CLI feature set already — see [INSTALL.md](INSTALL.md)), since the
Ollama/LM Studio/GPT4All backends are all routed through the OpenAI-compatible
client.

**Recommended models**, by task — smaller/faster for summarization, a model
with strong JSON adherence for structured extraction:

| Task | Recommended | Alternatives |
|---|---|---|
| Session summaries (episodic) | `qwen2.5:3b-instruct` | `llama3.2:3b`, `phi3.5:3.8b-mini-instruct` (lowest RAM) |
| Semantic fact distillation | `qwen2.5:3b-instruct` | `llama3.2:3b` |
| Entity/relation extraction, procedural workflow synthesis | `qwen2.5:7b-instruct` | `qwen2.5:14b-instruct` (accuracy), `llama3.1:8b-instruct` |

`qwen2.5` is favored for extraction/synthesis because it's the most reliable
of the small local models at sticking to the requested JSON shape — both
tasks above are JSON-only prompts.

Each memory still goes through no-LLM-required paths by default
(deterministic edge building, extractive session summaries, embedding-cluster
semantic facts, frequent-sequence procedural mining — see
[HOW_IT_WORKS.md](HOW_IT_WORKS.md)); the LLM path only activates per-call via
`remember`'s `llm_assist: true` once both `llm.enabled` and
`enrichment.enabled` are set.

### Local cross-encoder reranker (optional)

A reranking pass over recall results — off by default, since it's an extra
model + inference pass per query (a deliberate trade against low-end-machine
performance; the deterministic hybrid fusion remains the baseline). Needs the
binary built with `--features reranker`:

```toml
[retrieval]
rerank_enabled = true
reranker_model_id = "jinaai/jina-reranker-v2-base-multilingual"  # default
```

`BAAI/bge-reranker-v2-m3` is the accuracy-over-speed alternative. The model
downloads once (cached by `hf_hub`) on first use.

## Environment variables

| Variable | Default | Used by | Purpose |
|---|---|---|---|
| `PONEGLYPH_CONTEXT_TOKENS` | `600` | sessionstart.sh | Context injection token budget |

## Security notes

- Hooks always exit 0 — a missing `poneglyph` binary never blocks your agent
- Commands run in background (`&`) so they don't block the agent
- Content truncated to 4000 chars
- MCP server binds `127.0.0.1` only (loopback, no network exposure) unless
  you've explicitly reconfigured it; use `--stdio` to avoid binding a port
  at all

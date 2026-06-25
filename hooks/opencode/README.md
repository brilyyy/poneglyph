# poneglyph — opencode plugin

MCP + HTTP hybrid capture. Uses MCP for structured memory queries
(remember/recall/context) and HTTP `/ingest` for fire-and-forget event capture
(same pipeline as the Claude Code hooks).

## Install

```sh
# Project-level (recommended):
mkdir -p .opencode/plugins
cp poneglyph.ts .opencode/plugins/

# Or global:
mkdir -p ~/.config/opencode/plugins
cp poneglyph.ts .opencode/plugins/
```

Requires `poneglyph mcp` to be configured as an MCP server in your
`opencode.json` (done automatically by `poneglyph init`).

## What it does

### Injection (context → agent)

| Hook | Behavior |
|---|---|
| `experimental.chat.system.transform` | Injects project context via `get_project_context` into every system prompt |
| `experimental.session.compacting` | Injects project context into compaction prompt (fallback) |

### Capture (agent → poneglyph)

| Hook | Event | Memory type |
|---|---|---|
| `session.created` | Session start | episodic |
| `session.idle` | Triggers consolidate (debounced 30min) | — |
| `session.deleted` | Triggers consolidate | — |
| `session.error` | Error details | episodic |
| `tool.execute.before` | File enrichment (Read/Write/Edit) | — |
| `tool.execute.after` | Tool execution + file edits | code_context |
| `message.updated` | User/assistant messages | episodic |
| `message.removed` | Message deletion | episodic |
| `permission.asked` | Permission request | episodic |
| `permission.replied` | Permission reply | episodic |
| `todo.updated` | Task state changes | procedural |
| `command.executed` | Shell commands | code_context |
| `file.watcher.updated` | File watcher changes | code_context |

### File enrichment

Before file-touching tools (Write/Edit/MultiEdit), the plugin queries
`/api/enrich` for relevant memories and codegraph nodes for that file, so the
agent has context about what's already known.

## Environment

| Variable | Default | Description |
|---|---|---|
| `PONEGLYPH_PORT` | `27271` | Engine HTTP port |
| `PONEGLYPH_DASHBOARD_TOKEN` | (empty) | Auth token if configured |

## Caveat

opencode's plugin API changes between versions — if capture doesn't work,
check the hook names against your installed version's plugin docs
(https://opencode.ai/docs/plugins/). The plugin never blocks the agent:
all failures are swallowed.

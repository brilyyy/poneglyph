# poneglyph — opencode plugin

Pure-MCP plugin: uses poneglyph's MCP tools directly, no HTTP dependency.

## Install

```sh
# Project-level (recommended):
mkdir -p .opencode/plugins
cp poneglyph.ts .opencode/plugins/

# Or global:
mkdir -p ~/.config/opencode/plugins
cp poneglyph.ts ~/.config/opencode/plugins/
```

Requires `poneglyph mcp` to be configured as an MCP server in your
`opencode.json` (done automatically by `poneglyph init`).

## What it does

| Hook | Behavior |
|---|---|
| `experimental.session.compacting` | Injects project context from `poneglyph_get_project_context` into compaction prompt |
| `tool.execute.after` | Logs tool executions via `client.app.log()` |
| `message.updated` | Logs user/assistant messages via `client.app.log()` |

## Environment

No environment variables needed — the plugin communicates through the MCP
server, not HTTP.

## Caveat

opencode's plugin API changes between versions — if capture doesn't work,
check the hook names against your installed version's plugin docs
(https://opencode.ai/docs/plugins/). The plugin never blocks the agent:
all failures are swallowed.

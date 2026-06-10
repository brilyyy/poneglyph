# poneglyph — opencode passive-capture plugin

POSTs opencode tool executions and assistant messages to the local poneglyph
`/ingest` endpoint.

## Install

```sh
# Project-level (recommended):
mkdir -p .opencode/plugins
cp poneglyph.ts .opencode/plugins/

# Or global:
mkdir -p ~/.config/opencode/plugins
cp poneglyph.ts ~/.config/opencode/plugins/
```

Requires `poneglyph serve` running (HTTP on `127.0.0.1:3742` by default).

## What gets captured

| Event | What | Memory type |
|---|---|---|
| `tool.execute.after` | Write tool executions (Edit, Write, Bash, etc.) | `code_context` |
| `message.updated` | Assistant responses | `episodic` |

Read-only tools (read, glob, grep, list, todoread, webfetch) are skipped.

## Environment

| Variable | Default | Purpose |
|---|---|---|
| `PONEGLYPH_PORT` | `3742` | HTTP port of `poneglyph serve` |
| `PONEGLYPH_TOKEN` | unset | Bearer token, required if `server.api_token` is set |

## Caveat

opencode's plugin API changes between versions — if capture doesn't work,
check the hook names (`tool.execute.after`, `message.updated`) against your
installed version's plugin docs (https://opencode.ai/docs/plugins/).
The plugin never blocks the agent: all failures are swallowed with a 2s fetch
timeout.

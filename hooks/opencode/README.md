# poneglyph — opencode passive-capture plugin

POSTs opencode tool executions and user/assistant messages to the local
poneglyph `/ingest` endpoint, and logs project memory to the console on load.

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
| `message.updated` (role=user) | User prompts | `episodic` |
| `message.updated` (role=assistant) | Assistant responses | `episodic` |

Read-only tools (read, glob, grep, list, todoread, webfetch) are skipped.

## Session-start context (best-effort)

On plugin load, a one-shot `GET /api/context` fetches ranked project memory
and logs it to the console (`[poneglyph] project memory: ...`). This is a
partial parity feature with Claude Code's SessionStart hook — opencode's
plugin factory has no return-value injection point at load time, so this
context cannot be placed directly into the model's context the way Claude
Code's hook stdout can. It's visible to a human watching the terminal, not
automatically read by the agent. If opencode's plugin API ever exposes a
hook whose return value is injected into the conversation, switch to that.

## Environment

| Variable | Default | Purpose |
|---|---|---|
| `PONEGLYPH_PORT` | `3742` | HTTP port of `poneglyph serve` |
| `PONEGLYPH_TOKEN` | unset | Bearer token, required if `server.api_token` is set |
| `PONEGLYPH_CONTEXT_TOKENS` | `600` | Token budget for the session-start context fetch |

## Caveat

opencode's plugin API changes between versions — if capture doesn't work,
check the hook names (`tool.execute.after`, `message.updated`) against your
installed version's plugin docs (https://opencode.ai/docs/plugins/).
The plugin never blocks the agent: all failures are swallowed with a 2s fetch
timeout.

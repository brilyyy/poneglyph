# poneglyph — Claude Code passive-capture hooks

Thin shims that POST/GET the `poneglyph mcp` engine (a persistent HTTP
daemon on `127.0.0.1:27271` by default) to store and fetch session events.
Falls back to a direct `poneglyph` CLI call if the engine isn't reachable,
so capture still works (just slower, in-process) without it running.

## Requirements

- `poneglyph mcp` running (`poneglyph wire claude-code` reminds you)
- `curl` and `jq` on PATH
- `poneglyph` on PATH (used only as the engine-down fallback)

## Install

```sh
# Automatic (recommended):
poneglyph wire claude-code

# Manual:
mkdir -p ~/.config/poneglyph/hooks
cp posttooluse.sh userpromptsubmit.sh sessionstart.sh stop.sh ~/.config/poneglyph/hooks/
chmod +x ~/.config/poneglyph/hooks/*.sh
```

Then merge `settings.json.example` into `~/.claude/settings.json` (or a
project's `.claude/settings.json`). The `matcher` controls which tools get
captured; the scripts additionally skip read-only tools as a backstop.

## Hooks

| Hook | Event | What it captures |
|---|---|---|
| `posttooluse.sh` | PostToolUse | Tool executions as `code_context` memories via `POST /ingest` |
| `userpromptsubmit.sh` | UserPromptSubmit | User prompts as `episodic` memories via `POST /ingest` |
| `stop.sh` | Stop | Last assistant message via `POST /ingest`, then a session summary via `poneglyph session-summary` (no REST equivalent yet) |
| `sessionstart.sh` | SessionStart | Injects project context via `GET /api/context` (read-only) |

## Environment

| Variable | Default | Purpose |
|---|---|---|
| `PONEGLYPH_PORT` | `27271` | Port the `poneglyph mcp` engine listens on |
| `PONEGLYPH_DASHBOARD_TOKEN` | unset | Bearer token, only needed if `dashboard.token` is configured |
| `PONEGLYPH_CONTEXT_TOKENS` | `600` | Token budget for SessionStart context injection |

## Session context injection (sessionstart.sh)

On every session start, the project's most relevant memories (ranked by
importance × recency × access) are injected as context — capped at
`PONEGLYPH_CONTEXT_TOKENS`, computed entirely locally, **zero LLM calls**.
Unknown projects inject nothing.

## What gets captured

- **Tool executions:** Every tool call (Bash, Edit, Write, MultiEdit, Agent,
  Task, NotebookEdit, etc.) is stored as a `code_context` memory tagged with
  the tool name and project.
- **User prompts:** Each prompt you type is stored as an `episodic` memory.
- **Assistant messages:** Claude's final response is stored as an
  `episodic` memory via the Stop hook.
- **Session context:** On new sessions, your project's most relevant memories
  are injected into Claude's context window (zero LLM calls).

Read-only tools (Read, Glob, Grep, TodoWrite, WebSearch, WebFetch) are
skipped to reduce noise.

## Behavior guarantees

- Always exits 0 — engine down or a missing `poneglyph` binary never blocks
  Claude Code.
- Engine-down fallback: if curling the engine fails, each hook falls back to
  a direct `poneglyph remember`/`context` CLI call.
- Network calls run in background (`&`) so they don't block the agent.
- Content truncated to 4000 chars.
- Tool output truncated to 2000 chars within the content.
- The code-graph self-heal trigger (`posttooluse.sh`) and session-summary
  *generation* (`stop.sh`) stay CLI calls — no REST endpoint for those yet.

## Verify

```sh
# With the engine running (poneglyph mcp):
echo '{"tool_name":"Bash","tool_input":{"command":"ls"},"tool_output":"file1.txt\nfile2.txt","cwd":"'$PWD'","session_id":"test"}' \
  | ~/.config/poneglyph/hooks/posttooluse.sh
poneglyph recall "ls" --limit 3
```

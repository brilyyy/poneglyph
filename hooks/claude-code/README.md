# poneglyph — Claude Code passive-capture hooks

Thin shims that call the `poneglyph` CLI to store session events as memories.
No HTTP server needed — just `poneglyph` on PATH.

## Requirements

- `poneglyph` on PATH
- `jq` on PATH

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
| `posttooluse.sh` | PostToolUse | Tool executions as `code_context` memories via `poneglyph remember` |
| `userpromptsubmit.sh` | UserPromptSubmit | User prompts as `episodic` memories via `poneglyph remember` |
| `stop.sh` | Stop | Last assistant message from transcript as `episodic` memories via `poneglyph remember` |
| `sessionstart.sh` | SessionStart | Injects project context via `poneglyph context` (read-only) |

## Environment

| Variable | Default | Purpose |
|---|---|---|
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

- Always exits 0 — a missing `poneglyph` binary never blocks Claude Code.
- Commands run in background (`&`) so they don't block the agent.
- Content truncated to 4000 chars.
- Tool output truncated to 2000 chars within the content.

## Verify

```sh
echo '{"tool_name":"Bash","tool_input":{"command":"ls"},"tool_output":"file1.txt\nfile2.txt","cwd":"'$PWD'","session_id":"test"}' \
  | ~/.config/poneglyph/hooks/posttooluse.sh
poneglyph recall "ls" --limit 3
```

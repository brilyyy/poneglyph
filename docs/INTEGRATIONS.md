# Integrations

poneglyph integrates with Claude Code through hooks, MCP tools, and a skill.

## Claude Code

### Quick setup

```sh
poneglyph wire claude-code
```

This configures:
- MCP server (`poneglyph mcp`) in `~/.claude.json`
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
| `stop.sh` | Stop | `episodic` | Last assistant message |
| `sessionstart.sh` | SessionStart | *(read-only)* | Injects project context via `poneglyph context` |

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

Claude Desktop connects to poneglyph via MCP stdio. Add to your Claude Desktop
config (`~/Library/Application Support/Claude/claude_desktop_config.json` on
macOS):

```json
{
  "mcpServers": {
    "poneglyph": {
      "command": "/path/to/poneglyph",
      "args": ["mcp"]
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

## Environment variables

| Variable | Default | Used by | Purpose |
|---|---|---|---|
| `PONEGLYPH_CONTEXT_TOKENS` | `600` | sessionstart.sh | Context injection token budget |

## Security notes

- Hooks always exit 0 — a missing `poneglyph` binary never blocks your agent
- Commands run in background (`&`) so they don't block the agent
- Content truncated to 4000 chars
- MCP server communicates over stdio (no network exposure)

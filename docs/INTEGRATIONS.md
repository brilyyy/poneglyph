# Integrations

poneglyph integrates with Claude Code, Claude Desktop, and OpenCode through
hooks and MCP tools.

## Claude Code (hooks)

Hooks auto-capture tool executions, user prompts, and assistant messages as
passive memories. Session context is injected on new sessions.

Hooks POST to the `/ingest` HTTP endpoint, so they need **`poneglyph
viewer`** running in the background (not `poneglyph serve`, which is
MCP-only). If you also want MCP tools (`remember`/`recall`/`codegraph_query`)
in Claude Code, run `poneglyph serve` as well — they're independent
processes against the same database.

### Install hooks

```sh
mkdir -p ~/.config/poneglyph/hooks
cp hooks/claude-code/*.sh ~/.config/poneglyph/hooks/
chmod +x ~/.config/poneglyph/hooks/*.sh
```

### Install the skill

Teaches Claude Code when to reach for `remember`/`recall`/`get_project_context`
and `codegraph_query`/`codegraph_blast_radius` instead of ad-hoc grepping:

```sh
mkdir -p ~/.claude/skills/poneglyph
cp hooks/poneglyph/SKILL.md ~/.claude/skills/poneglyph/SKILL.md
```

(Or project-scoped: `.claude/skills/poneglyph/SKILL.md`.)

### Inject usage rules into CLAUDE.md / AGENTS.md / .cursorrules

Opt-in — `poneglyph init` never touches these files unless asked:

```sh
poneglyph init --inject-rules
```

For each of `CLAUDE.md`, `AGENTS.md`, `.cursorrules` that already exists in
the current directory, idempotently inserts/replaces a fenced
`<!-- poneglyph:start --> ... <!-- poneglyph:end -->` block with a condensed
usage summary. Never creates a file that doesn't already exist; re-running
replaces the block in place instead of duplicating it.

### Configure Claude Code

Merge into `~/.claude/settings.json` (or a project's `.claude/settings.json`):

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Edit|Write|MultiEdit|Bash|NotebookEdit|Agent|Task",
        "hooks": [{ "type": "command", "command": "$HOME/.config/poneglyph/hooks/posttooluse.sh", "timeout": 5 }]
      }
    ],
    "UserPromptSubmit": [
      {
        "hooks": [{ "type": "command", "command": "$HOME/.config/poneglyph/hooks/userpromptsubmit.sh", "timeout": 5 }]
      }
    ],
    "Stop": [
      {
        "hooks": [{ "type": "command", "command": "$HOME/.config/poneglyph/hooks/stop.sh", "timeout": 10 }]
      }
    ],
    "SessionStart": [
      {
        "hooks": [{ "type": "command", "command": "$HOME/.config/poneglyph/hooks/sessionstart.sh", "timeout": 5 }]
      }
    ]
  }
}
```

### What gets captured

| Hook | Event | Memory type | Content |
|---|---|---|---|
| `posttooluse.sh` | PostToolUse | `code_context` | Tool name + input + output. Also debounce-triggers `poneglyph graph update` (skip if triggered <10s ago) after `Edit`/`Write`/`MultiEdit` on a recognized source extension, so the code graph self-heals without a separate `graph watch` process. |
| `userpromptsubmit.sh` | UserPromptSubmit | `episodic` | User prompt text |
| `stop.sh` | Stop | `episodic` | Last assistant message |
| `sessionstart.sh` | SessionStart | *(read-only)* | Injects project context |

### Session context injection

On every new session, `sessionstart.sh` fetches your project's most relevant
memories (ranked by importance × recency × access) and injects them into
Claude's context. This is zero LLM calls — computed entirely locally.

Control the budget with `PONEGLYPH_CONTEXT_TOKENS` (default 600).

### Verify

```sh
# Start the viewer (hooks POST captures here)
poneglyph viewer &

# Use Claude Code in a project — tool calls are captured automatically

# Check captured memories
poneglyph recall "recent work" --limit 5

# Or open the viewer
open http://127.0.0.1:3742/memories
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
      "args": ["serve"]
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

## OpenCode (plugin)

The plugin auto-captures tool executions and assistant messages.

### Install plugin

```sh
# Project-level
mkdir -p .opencode/plugins
cp hooks/opencode/poneglyph.ts .opencode/plugins/

# Or global
mkdir -p ~/.config/opencode/plugins
cp hooks/opencode/poneglyph.ts ~/.config/opencode/plugins/
```

### What gets captured

| Event | Memory type |
|---|---|
| `tool.execute.after` (write tools) | `code_context` |
| `message.updated` (assistant messages) | `episodic` |

### Environment

Same as Claude Code: `PONEGLYPH_PORT` (default 3742), `PONEGLYPH_TOKEN` if
`server.api_token` is set.

## Environment variables

| Variable | Default | Used by | Purpose |
|---|---|---|---|
| `PONEGLYPH_PORT` | `3742` | All hooks | HTTP port of `poneglyph viewer` |
| `PONEGLYPH_TOKEN` | unset | All hooks | Bearer token for non-loopback |
| `PONEGLYPH_CONTEXT_TOKENS` | `600` | sessionstart.sh | Context injection token budget |

## Security notes

- Hooks always exit 0 — a dead `poneglyph viewer` never blocks your agent
- 2-second curl/fetch timeout on all hook requests
- Content truncated to 4000 chars before POSTing
- The HTTP server binds to `127.0.0.1` by default
- If you bind to a non-loopback address, `server.api_token` is required

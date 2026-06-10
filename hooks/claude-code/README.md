# poneglyph — Claude Code passive-capture hooks

Thin shims that POST Claude Code session events to the local poneglyph
`/ingest` endpoint (PRD §8.9). No business logic lives here — the server maps
events to passive memories.

## Requirements

- `poneglyph serve` running (HTTP on `127.0.0.1:3742` by default)
- `jq` and `curl` on PATH

## Install

```sh
mkdir -p ~/.config/poneglyph/hooks
cp posttooluse.sh userpromptsubmit.sh ~/.config/poneglyph/hooks/
chmod +x ~/.config/poneglyph/hooks/*.sh
```

Then merge `settings.json.example` into `~/.claude/settings.json` (or a
project's `.claude/settings.json`). The `matcher` controls which tools get
captured; the scripts additionally skip read-only tools as a backstop.

## Environment

| Variable | Default | Purpose |
|---|---|---|
| `PONEGLYPH_PORT` | `3742` | HTTP port of `poneglyph serve` |
| `PONEGLYPH_TOKEN` | unset | Bearer token, required if `server.api_token` is set |

## Behavior guarantees

- Always exits 0 — a dead or missing poneglyph server never blocks Claude Code.
- 2-second curl timeout.
- Content truncated to 4000 chars before POSTing.

## Verify

```sh
echo '{"tool_name":"Bash","tool_input":{"command":"ls"},"cwd":"'$PWD'","session_id":"test"}' \
  | ~/.config/poneglyph/hooks/posttooluse.sh
poneglyph recall "ls" --limit 3
```

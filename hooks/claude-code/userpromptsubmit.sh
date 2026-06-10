#!/usr/bin/env bash
# Claude Code UserPromptSubmit → poneglyph /ingest. Thin shim: never blocks, never fails.
set -u
INPUT=$(cat) || exit 0
PORT="${PONEGLYPH_PORT:-3742}"

PROMPT=$(printf '%s' "$INPUT" | jq -r '.prompt // empty' 2>/dev/null) || exit 0
[ -z "$PROMPT" ] && exit 0

PAYLOAD=$(printf '%s' "$INPUT" | jq -c '{
  event: "user_message",
  client: "claude-code",
  project_path: (.cwd // empty),
  content: ((.prompt // "") | .[0:4000]),
  metadata: { session_id: (.session_id // null) }
}' 2>/dev/null) || exit 0
[ -z "$PAYLOAD" ] && exit 0

curl -s -m 2 -o /dev/null -X POST "http://127.0.0.1:${PORT}/ingest" \
  -H 'Content-Type: application/json' \
  ${PONEGLYPH_TOKEN:+-H "Authorization: Bearer ${PONEGLYPH_TOKEN}"} \
  -d "$PAYLOAD" 2>/dev/null

exit 0  # always succeed — never block Claude Code

#!/usr/bin/env bash
# Claude Code PostToolUse → poneglyph /ingest. Thin shim: never blocks, never fails.
set -u
INPUT=$(cat) || exit 0
PORT="${PONEGLYPH_PORT:-3742}"

# Skip read-only / noisy tools. Tool selection also configurable via the
# matcher in settings.json; this is the backstop.
TOOL=$(printf '%s' "$INPUT" | jq -r '.tool_name // empty' 2>/dev/null) || exit 0
case "$TOOL" in
  Read|Glob|Grep|TodoWrite|Task|WebSearch|WebFetch|"") exit 0 ;;
esac

PAYLOAD=$(printf '%s' "$INPUT" | jq -c '{
  event: "tool_use",
  client: "claude-code",
  project_path: (.cwd // empty),
  tool: .tool_name,
  content: ((.tool_name + " " + ((.tool_input // {}) | tostring)) | .[0:4000]),
  metadata: { session_id: (.session_id // null) }
}' 2>/dev/null) || exit 0
[ -z "$PAYLOAD" ] && exit 0

curl -s -m 2 -o /dev/null -X POST "http://127.0.0.1:${PORT}/ingest" \
  -H 'Content-Type: application/json' \
  ${PONEGLYPH_TOKEN:+-H "Authorization: Bearer ${PONEGLYPH_TOKEN}"} \
  -d "$PAYLOAD" 2>/dev/null

exit 0  # always succeed — never block Claude Code

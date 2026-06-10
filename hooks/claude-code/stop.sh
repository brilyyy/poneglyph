#!/usr/bin/env bash
# Claude Code Stop → poneglyph /ingest.
# Captures the last assistant message from the transcript as an episodic memory.
# Thin shim: never blocks, never fails.
set -u
INPUT=$(cat) || exit 0
PORT="${PONEGLYPH_PORT:-3742}"

TRANSCRIPT=$(printf '%s' "$INPUT" | jq -r '.transcript_path // empty' 2>/dev/null) || exit 0
[ -z "$TRANSCRIPT" ] && exit 0
[ -f "$TRANSCRIPT" ] || exit 0

# Extract the last assistant message from the JSONL transcript.
# Each line is a JSON object with role and content fields.
MSG=$(tail -r "$TRANSCRIPT" 2>/dev/null | while IFS= read -r line; do
  ROLE=$(printf '%s' "$line" | jq -r '.role // empty' 2>/dev/null)
  if [ "$ROLE" = "assistant" ]; then
    printf '%s' "$line" | jq -r '.content // empty' 2>/dev/null
    break
  fi
done) || exit 0

[ -z "$MSG" ] && exit 0

# Truncate to 4000 chars.
MSG="${MSG:0:4000}"

PAYLOAD=$(printf '%s' "$INPUT" | jq -c --arg content "$MSG" '{
  event: "assistant_message",
  client: "claude-code",
  project_path: (.cwd // empty),
  content: $content,
  metadata: { session_id: (.session_id // null) }
}' 2>/dev/null) || exit 0
[ -z "$PAYLOAD" ] && exit 0

curl -s -m 2 -o /dev/null -X POST "http://127.0.0.1:${PORT}/ingest" \
  -H 'Content-Type: application/json' \
  ${PONEGLYPH_TOKEN:+-H "Authorization: Bearer ${PONEGLYPH_TOKEN}"} \
  -d "$PAYLOAD" 2>/dev/null

exit 0  # always succeed — never block Claude Code

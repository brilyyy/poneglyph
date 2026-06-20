#!/usr/bin/env bash
# Claude Code Stop — capture last assistant message via the poneglyph engine.
# Thin shim: never blocks, never fails.
set -u
INPUT=$(cat) || exit 0

TRANSCRIPT=$(printf '%s' "$INPUT" | jq -r '.transcript_path // empty' 2>/dev/null) || exit 0
[ -z "$TRANSCRIPT" ] && exit 0
[ -f "$TRANSCRIPT" ] || exit 0

CWD=$(printf '%s' "$INPUT" | jq -r '.cwd // empty' 2>/dev/null) || exit 0

PORT="${PONEGLYPH_PORT:-27271}"
BASE="http://127.0.0.1:${PORT}"

# Extract the last assistant message from the JSONL transcript.
MSG=$(tail -r "$TRANSCRIPT" 2>/dev/null | while IFS= read -r line; do
  ROLE=$(printf '%s' "$line" | jq -r '.role // empty' 2>/dev/null)
  if [ "$ROLE" = "assistant" ]; then
    printf '%s' "$line" | jq -r '.content // empty' 2>/dev/null
    break
  fi
done) || exit 0

if [ -n "$MSG" ]; then
  # Truncate to 4000 chars.
  CONTENT="assistant_message: ${MSG:0:4000}"
  (
    BODY=$(jq -n --arg content "$CONTENT" --arg project "$CWD" \
      '{event: "assistant_message", client: "claude-code", content: $content}
       + (if $project == "" then {} else {project_path: $project} end)') 2>/dev/null
    if ! curl -sf -m 3 \
        -H "Authorization: Bearer ${PONEGLYPH_DASHBOARD_TOKEN:-}" \
        -H "Content-Type: application/json" \
        -d "$BODY" "$BASE/ingest" >/dev/null 2>&1; then
      # Engine not running — fall back to a direct CLI write.
      command -v poneglyph >/dev/null 2>&1 && poneglyph remember "$CONTENT" \
        --type episodic \
        --importance 0.5 \
        ${CWD:+--project "$CWD"} \
        >/dev/null 2>&1
    fi
  ) &
fi

# Generate a session summary (extractive, or LLM-backed when configured).
# ponytail: /api/session-summary is read-only (latest existing summary) —
# generation still goes through the CLI; add a POST endpoint if this needs
# to drop the CLI dependency too.
command -v poneglyph >/dev/null 2>&1 && poneglyph session-summary \
  ${CWD:+--project "$CWD"} \
  >/dev/null 2>&1 &

exit 0  # always succeed — never block Claude Code

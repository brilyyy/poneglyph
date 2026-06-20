#!/usr/bin/env bash
# Claude Code UserPromptSubmit — store user prompt via the poneglyph engine.
# Thin shim: never blocks, never fails.
set -u
INPUT=$(cat) || exit 0

PROMPT=$(printf '%s' "$INPUT" | jq -r '.prompt // empty' 2>/dev/null) || exit 0
[ -z "$PROMPT" ] && exit 0

CWD=$(printf '%s' "$INPUT" | jq -r '.cwd // empty' 2>/dev/null) || exit 0

PORT="${PONEGLYPH_PORT:-27271}"
BASE="http://127.0.0.1:${PORT}"

# Truncate to 4000 chars.
CONTENT="user_message: ${PROMPT:0:4000}"

(
  BODY=$(jq -n --arg content "$CONTENT" --arg project "$CWD" \
    '{event: "user_message", client: "claude-code", content: $content}
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

exit 0  # always succeed — never block Claude Code

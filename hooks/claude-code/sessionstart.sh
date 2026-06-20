#!/usr/bin/env bash
# Claude Code SessionStart — inject project context from the poneglyph engine.
# stdout becomes injected session context. Zero LLM calls.
set -u
INPUT=$(cat) || exit 0
TOKENS="${PONEGLYPH_CONTEXT_TOKENS:-600}"
PORT="${PONEGLYPH_PORT:-27271}"
BASE="http://127.0.0.1:${PORT}"

CWD=$(printf '%s' "$INPUT" | jq -r '.cwd // empty' 2>/dev/null) || exit 0
[ -z "$CWD" ] && exit 0

ENC_CWD=$(jq -rn --arg v "$CWD" '$v|@uri' 2>/dev/null) || ENC_CWD="$CWD"

if CONTEXT_JSON=$(curl -sf -m 3 \
    -H "Authorization: Bearer ${PONEGLYPH_DASHBOARD_TOKEN:-}" \
    "$BASE/api/context?project_path=${ENC_CWD}&max_tokens=${TOKENS}" 2>/dev/null); then
  printf '%s' "$CONTEXT_JSON" | jq -r '.context // empty' 2>/dev/null
else
  # Engine not running — fall back to a direct (slower, in-process) CLI call.
  command -v poneglyph >/dev/null 2>&1 && poneglyph context --project "$CWD" --max-tokens "$TOKENS" 2>/dev/null
fi

# Show last session summary if available.
SUMMARY=""
if SUMMARY_JSON=$(curl -sf -m 3 \
    -H "Authorization: Bearer ${PONEGLYPH_DASHBOARD_TOKEN:-}" \
    "$BASE/api/session-summary?project_path=${ENC_CWD}" 2>/dev/null); then
  SUMMARY=$(printf '%s' "$SUMMARY_JSON" | jq -r '.content // empty' 2>/dev/null)
else
  command -v poneglyph >/dev/null 2>&1 && SUMMARY=$(poneglyph session-summary --latest --project "$CWD" 2>/dev/null)
fi
if [ -n "$SUMMARY" ]; then
  echo ""
  echo "## Last session summary"
  echo "$SUMMARY"
fi

exit 0  # always succeed — never block session start

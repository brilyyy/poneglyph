#!/usr/bin/env bash
# Claude Code SessionStart — inject project context via poneglyph CLI.
# stdout becomes injected session context. Zero LLM calls.
set -u
INPUT=$(cat) || exit 0
TOKENS="${PONEGLYPH_CONTEXT_TOKENS:-600}"

CWD=$(printf '%s' "$INPUT" | jq -r '.cwd // empty' 2>/dev/null) || exit 0
[ -z "$CWD" ] && exit 0

if ! command -v poneglyph >/dev/null 2>&1; then
  exit 0
fi

poneglyph context --project "$CWD" --max-tokens "$TOKENS" 2>/dev/null || exit 0

# Show last session summary if available.
SUMMARY=$(poneglyph session-summary --latest --project "$CWD" 2>/dev/null) || true
if [ -n "$SUMMARY" ]; then
  echo ""
  echo "## Last session summary"
  echo "$SUMMARY"
fi

exit 0  # always succeed — never block session start

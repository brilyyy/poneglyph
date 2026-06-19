#!/usr/bin/env bash
# Claude Code UserPromptSubmit — store user prompt via poneglyph CLI.
# Thin shim: never blocks, never fails.
set -u
INPUT=$(cat) || exit 0

PROMPT=$(printf '%s' "$INPUT" | jq -r '.prompt // empty' 2>/dev/null) || exit 0
[ -z "$PROMPT" ] && exit 0

CWD=$(printf '%s' "$INPUT" | jq -r '.cwd // empty' 2>/dev/null) || exit 0

if ! command -v poneglyph >/dev/null 2>&1; then
  exit 0
fi

# Truncate to 4000 chars.
CONTENT="user_message: ${PROMPT:0:4000}"

poneglyph remember "$CONTENT" \
  --type episodic \
  --importance 0.5 \
  ${CWD:+--project "$CWD"} \
  >/dev/null 2>&1 &

exit 0  # always succeed — never block Claude Code

#!/usr/bin/env bash
# Claude Code Stop — capture last assistant message via poneglyph CLI.
# Thin shim: never blocks, never fails.
set -u
INPUT=$(cat) || exit 0

TRANSCRIPT=$(printf '%s' "$INPUT" | jq -r '.transcript_path // empty' 2>/dev/null) || exit 0
[ -z "$TRANSCRIPT" ] && exit 0
[ -f "$TRANSCRIPT" ] || exit 0

if ! command -v poneglyph >/dev/null 2>&1; then
  exit 0
fi

CWD=$(printf '%s' "$INPUT" | jq -r '.cwd // empty' 2>/dev/null) || exit 0

# Extract the last assistant message from the JSONL transcript.
MSG=$(tail -r "$TRANSCRIPT" 2>/dev/null | while IFS= read -r line; do
  ROLE=$(printf '%s' "$line" | jq -r '.role // empty' 2>/dev/null)
  if [ "$ROLE" = "assistant" ]; then
    printf '%s' "$line" | jq -r '.content // empty' 2>/dev/null
    break
  fi
done) || exit 0

[ -z "$MSG" ] && exit 0

# Truncate to 4000 chars.
CONTENT="assistant_message: ${MSG:0:4000}"

poneglyph remember "$CONTENT" \
  --type episodic \
  --importance 0.5 \
  ${CWD:+--project "$CWD"} \
  >/dev/null 2>&1 &

# Generate session summary (extractive, no LLM needed).
poneglyph session-summary \
  ${CWD:+--project "$CWD"} \
  >/dev/null 2>&1 &

exit 0  # always succeed — never block Claude Code

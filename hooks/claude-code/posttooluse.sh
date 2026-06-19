#!/usr/bin/env bash
# Claude Code PostToolUse — store tool execution via poneglyph CLI.
# Thin shim: never blocks, never fails.
set -u
INPUT=$(cat) || exit 0

# Skip read-only / noisy tools. Tool selection also configurable via the
# matcher in settings.json; this is the backstop.
TOOL=$(printf '%s' "$INPUT" | jq -r '.tool_name // empty' 2>/dev/null) || exit 0
case "$TOOL" in
  Read|Glob|Grep|TodoWrite|WebSearch|WebFetch|"") exit 0 ;;
esac

if ! command -v poneglyph >/dev/null 2>&1; then
  exit 0
fi

CWD=$(printf '%s' "$INPUT" | jq -r '.cwd // empty' 2>/dev/null) || exit 0

# Build content: tool name + input + output.
TOOL_INPUT=$(printf '%s' "$INPUT" | jq -r '.tool_input // {} | tostring' 2>/dev/null) || TOOL_INPUT='{}'
TOOL_OUTPUT=$(printf '%s' "$INPUT" | jq -r '.tool_output // "" | .[0:2000]' 2>/dev/null) || TOOL_OUTPUT=''
CONTENT="${TOOL} ${TOOL_INPUT} → ${TOOL_OUTPUT}"
CONTENT="${CONTENT:0:4000}"

poneglyph remember "$CONTENT" \
  --type code_context \
  --importance 0.5 \
  --tag "$TOOL" \
  ${CWD:+--project "$CWD"} \
  >/dev/null 2>&1 &

# Self-heal the code graph after source edits, so codegraph_query/
# blast_radius stay fresh without a separate `poneglyph graph watch`
# process. ponytail: debounced via a per-project mtime marker file (skip if
# triggered <10s ago) rather than a real queue — fine for "mostly fresh",
# `graph watch` is still the answer for guaranteed freshness.
case "$TOOL" in
  Edit|Write|MultiEdit)
    FILE_PATH=$(printf '%s' "$INPUT" | jq -r '.tool_input.file_path // empty' 2>/dev/null)
    case "$FILE_PATH" in
      *.rs|*.ts|*.tsx|*.js|*.jsx|*.mjs|*.cjs|*.py|*.go)
        if [ -n "$CWD" ]; then
          HASH=$(printf '%s' "$CWD" | (md5 -q 2>/dev/null || md5sum 2>/dev/null | cut -d' ' -f1))
          MARKER="${TMPDIR:-/tmp}/poneglyph-graph-debounce-${HASH}"
          LAST=$(stat -f %m "$MARKER" 2>/dev/null || stat -c %Y "$MARKER" 2>/dev/null || echo 0)
          if [ $(($(date +%s) - LAST)) -ge 10 ]; then
            touch "$MARKER" 2>/dev/null
            (poneglyph graph update "$CWD" >/dev/null 2>&1 &) 2>/dev/null
          fi
        fi
        ;;
    esac
    ;;
esac

exit 0  # always succeed — never block Claude Code

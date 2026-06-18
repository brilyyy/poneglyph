#!/usr/bin/env bash
# Claude Code SessionStart → poneglyph /api/context.
# stdout becomes injected session context: project memory, ranked by
# importance × recency × access, capped to a token budget. Zero LLM calls.
set -u
INPUT=$(cat) || exit 0
PORT="${PONEGLYPH_PORT:-3742}"
TOKENS="${PONEGLYPH_CONTEXT_TOKENS:-600}"

CWD=$(printf '%s' "$INPUT" | jq -r '.cwd // empty' 2>/dev/null) || exit 0
[ -z "$CWD" ] && exit 0

CTX=$(curl -s -m 2 -G "http://127.0.0.1:${PORT}/api/context" \
  --data-urlencode "project_path=${CWD}" \
  --data-urlencode "max_tokens=${TOKENS}" \
  ${PONEGLYPH_TOKEN:+-H "Authorization: Bearer ${PONEGLYPH_TOKEN}"} \
  2>/dev/null | jq -r '.context // empty' 2>/dev/null) || exit 0

[ -n "$CTX" ] && printf '%s\n' "$CTX"

# Nudge toward `graph init` if the code graph is empty — otherwise
# codegraph_query/blast_radius silently return nothing and look like the
# symbol doesn't exist, rather than "the graph was never built".
NODES=$(curl -s -m 2 "http://127.0.0.1:${PORT}/api/codegraph/stats" \
  ${PONEGLYPH_TOKEN:+-H "Authorization: Bearer ${PONEGLYPH_TOKEN}"} \
  2>/dev/null | jq -r '.nodes // 0' 2>/dev/null) || NODES=0
[ "$NODES" = "0" ] && printf '%s\n' "poneglyph: code graph is empty — run \`poneglyph graph init\` to enable codegraph_query/codegraph_blast_radius."

exit 0  # always succeed — never block session start

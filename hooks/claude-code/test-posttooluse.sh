#!/usr/bin/env bash
# Smoke test for the graph-update debounce in posttooluse.sh.
# Run manually: ./test-posttooluse.sh
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# Stub out the poneglyph binary so this runs offline.
cat > "$TMP/poneglyph" <<EOF
#!/usr/bin/env bash
echo "\$*" >> "$TMP/calls.log"
EOF
chmod +x "$TMP/poneglyph"
export PATH="$TMP:$PATH"
export TMPDIR="$TMP"

PAYLOAD='{"tool_name":"Edit","tool_input":{"file_path":"src/lib.rs"},"cwd":"/fake/project","session_id":"s1"}'

printf '%s' "$PAYLOAD" | "$HERE/posttooluse.sh"
printf '%s' "$PAYLOAD" | "$HERE/posttooluse.sh"  # immediate repeat — should be debounced
sleep 0.5  # the graph-update trigger is a detached background process

CALLS=$(wc -l < "$TMP/calls.log" 2>/dev/null || echo 0)
if [ "$CALLS" -ne 1 ]; then
  echo "FAIL: expected exactly 1 graph update trigger, got $CALLS"
  cat "$TMP/calls.log" 2>/dev/null || true
  exit 1
fi
echo "PASS: debounce triggered graph update exactly once for 2 rapid edits"

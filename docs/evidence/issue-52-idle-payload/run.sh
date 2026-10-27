#!/usr/bin/env bash
# Issue 52 §10 — the 52.5 capture procedure, rung by rung.
#
# Usage: docs/evidence/issue-52-idle-payload/run.sh r1|r2|r3
#
# Each rung records the literal command it runs and its exit status; the raw
# host response (if any) lands at $PROBE_OUT, which is printed before the run.
# Rungs are attempted in order; a rung that produces no payload is recorded as
# such, never skipped silently.
set -u

RUNG="${1:-r1}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
PROBE_DIR="$ROOT/.tmp/evidence/issue-52"
PROJECT="$PROBE_DIR/project"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
PROBE_OUT="$PROBE_DIR/idle-payload-$STAMP.json"

mkdir -p "$PROJECT/.opencode/plugin"
ln -sfn "$HERE/probe-plugin.mjs" "$PROJECT/.opencode/plugin/probe-plugin.mjs"
rm -f "$PROBE_OUT"

echo "PROBE_OUT=$PROBE_OUT"
cd "$PROJECT" || exit 1

case "$RUNG" in
  r1)
    echo 'command: PROBE_OUT='"$PROBE_OUT"' timeout 180 opencode run --print-logs --auto "reply with the single word ok"'
    PROBE_OUT="$PROBE_OUT" timeout 180 opencode run --print-logs --auto "reply with the single word ok"
    ;;
  r2)
    echo 'command: PROBE_OUT='"$PROBE_OUT"' timeout 180 opencode run --standalone --print-logs --auto "reply with the single word ok"'
    PROBE_OUT="$PROBE_OUT" timeout 180 opencode run --standalone --print-logs --auto "reply with the single word ok"
    ;;
  r3)
    echo 'command: opencode models'
    opencode models
    ;;
  *)
    echo "unknown rung: $RUNG" >&2
    exit 2
    ;;
esac

STATUS=$?
echo "exit=$STATUS"
if [ -f "$PROBE_OUT" ]; then
  echo "payload=$PROBE_OUT"
else
  echo "payload=NONE"
fi
exit "$STATUS"

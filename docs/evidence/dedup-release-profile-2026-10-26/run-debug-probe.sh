#!/bin/sh
set -u
root=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
ev="$root/docs/evidence/dedup-release-profile-$(date -u +%F)"
mkdir -p "$ev"
out="$ev/debug-probe-1k.txt"
cd "$root" || exit 1
printf '# debug profile probe started %s\n' "$(date -u +%FT%TZ)" > "$out"
printf '# command: DEDUP_MEASURE_RUNGS=100 cargo test -p mem-web --test dedup_scan_measure -- --ignored --nocapture --test-threads=1\n' >> "$out"
timeout -k 30 600 env DEDUP_MEASURE_RUNGS=100 \
    sh "$root/.tmp/scripts/cargo-serialized.sh" cargo test -p mem-web \
        --test dedup_scan_measure -- --ignored --nocapture --test-threads=1 >> "$out" 2>&1
rc=$?
printf '# exit=%s finished %s\n' "$rc" "$(date -u +%FT%TZ)" >> "$out"

#!/bin/sh
set -u
root=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
out="$root/docs/evidence/dedup-release-profile-$(date -u +%F)"
mkdir -p "$out"
printf '# evidence dir (UTC date): %s\n' "$out" >> "$out/resolved-dir.txt"
cd "$root" || exit 1
for i in 1 2 3; do
    f="$out/release-run-$i.txt"
    printf '# run %s started %s\n' "$i" "$(date -u +%FT%TZ)" > "$f"
    printf '# command: DEDUP_MEASURE_RUNGS=1000,5000,10000 DEDUP_MEASURE_EXPECT=release cargo test -p mem-web --release --test dedup_scan_measure -- --ignored --nocapture --test-threads=1\n' >> "$f"
    timeout -k 30 1800 env DEDUP_MEASURE_RUNGS=1000,5000,10000 DEDUP_MEASURE_EXPECT=release \
        sh "$root/.tmp/scripts/cargo-serialized.sh" cargo test -p mem-web --release \
            --test dedup_scan_measure -- --ignored --nocapture --test-threads=1 >> "$f" 2>&1
    rc=$?
    printf '# run %s exit=%s finished %s\n' "$i" "$rc" "$(date -u +%FT%TZ)" >> "$f"
    if [ "$rc" -ne 0 ] && ! grep -q '== end of ladder ==' "$f"; then
        printf '# ABORT: run %s killed (exit=%s) at the 1800 s time-box; no end-of-ladder marker.\n' "$i" "$rc" >> "$f"
        break
    fi
    if ! awk '$1 == 1000 && NF >= 6 { found = 1 } END { exit !found }' "$f"; then
        printf '# ABORT: no n=1000 row; the n=10000 rung was not reached. Nothing inferred below.\n' >> "$f"
        break
    fi
    # NOTE: `{ exit 0 } END { exit 1 }` does NOT work -- the rule's exit still runs END, whose exit 1 wins.
    # The flag form below is required: fire=1 on a slow row, then END exits 0, otherwise 1.
    if [ "$i" = 1 ] && awk '$1 == 1000 && NF >= 6 && $3 > 17.9 { fire = 1 } END { exit !fire }' "$f"; then
        printf '# ABORT: n=1000 exceeded the pre-registered 17.9 s gate (1800 / 100.0901); runs 2-3 not attempted.\n' >> "$f"
        break
    fi
done
pgrep -af 'dedup_scan_measure-' || printf '# no stray measurement process\n'

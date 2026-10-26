#!/bin/sh
# Aggregates release-run-*.txt; median of an odd count is the middle value.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
echo "# derived $(date -u +%FT%TZ)"
for n in 1000 5000 10000; do
    awk -v n="$n" '$1 == n && NF >= 6 { print $4 }' "$here"/release-run-*.txt \
        | sort -n \
        | awk -v n="$n" '{ v[NR] = $1 } END {
              if (NR == 0) { printf "n=%-6s runs=0 NO DATA\n", n; exit }
              printf "n=%-6s runs=%d min=%.1f median=%.1f max=%.1f spread=%.1f%%\n", \
                  n, NR, v[1], v[int((NR + 1) / 2)], v[NR], 100 * (v[NR] - v[1]) / v[1]
          }'
done
echo "# detected profiles:"
grep -h 'cargo profile' "$here"/release-run-*.txt "$here"/debug-probe-1k.txt || true
echo "# end-of-ladder markers:"
grep -c '== end of ladder ==' "$here"/release-run-*.txt || true

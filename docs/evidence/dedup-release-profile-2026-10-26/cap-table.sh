#!/bin/sh
# usage: ./cap-table.sh <c_iter_release_ns>
set -eu
c=$1
cap_dbg=740000
cap_rel=$(awk -v c="$c" 'BEGIN {
    raw = 10e9 / c;
    s = 10 ^ (int(log(raw) / log(10)) - 1);
    f = int(raw / s) * s;
    if (f < 250000) f = 250000;
    if (f > 1250000000) f = 1250000000;
    printf "%.0f", f }')
printf '%-22s %12s %12s %10s %9s\n' candidate cap t_at_cap_s coverage pct_of_50k
printf '%-22s %12d %12.2f %10.0f %8.1f%%\n' 'current (debug-derived)' "$cap_dbg" \
    "$(awk -v cap="$cap_dbg" -v c="$c" 'BEGIN { print cap * c / 1e9 }')" \
    "$(awk -v cap="$cap_dbg" 'BEGIN { print sqrt(2 * cap) }')" \
    "$(awk -v cap="$cap_dbg" 'BEGIN { print 100 * sqrt(2 * cap) / 50000 }')"
printf '%-22s %12d %12.2f %10.0f %8.1f%%\n' 'release-derived' "$cap_rel" \
    "$(awk -v cap="$cap_rel" -v c="$c" 'BEGIN { print cap * c / 1e9 }')" \
    "$(awk -v cap="$cap_rel" 'BEGIN { print sqrt(2 * cap) }')" \
    "$(awk -v cap="$cap_rel" 'BEGIN { print 100 * sqrt(2 * cap) / 50000 }')"
printf '# k = %.3f  (13487.9 / %s)\n' "$(awk -v c="$c" 'BEGIN { print 13487.9 / c }')" "$c"

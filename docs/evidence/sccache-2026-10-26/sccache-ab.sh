#!/usr/bin/env bash
# sccache A/B/C/D for this repo.
#
# Void ships no /usr/bin/time, so peak RSS is SAMPLED (ps, every 200 ms, summed
# over cargo/rustc/cc/clang/lld) rather than read from wait4(). Sampled maxima
# undershoot true peaks; they are comparable across the four runs, which is what
# the comparison needs. Wall time is exact.
#
# A  no sccache, cold          — today's cost
# B  sccache, cold             — the population cost (must not be much worse than A)
# C  sccache, clean, warm      — the claim
# D  second empty target dir   — the fleet case: a fresh worktree against a shared cache
set -u
cd /home/vaz/Code/Personal/Rust/opencode-agents-memory || exit 1

SER="sh /home/vaz/Code/Personal/Rust/opencode-agents-memory/.tmp/scripts/cargo-serialized.sh"
OUT=.tmp/scratch/sccache-ab.txt
# Temp files live under the project scratch root: shell writes to /tmp are
# denied by this environment's permission layer.
mkdir -p .tmp/scratch
MON=$(mktemp .tmp/scratch/sccmon.XXXXXX)
CASE=$(mktemp .tmp/scratch/scccase.XXXXXX)
trap 'rm -f "$MON" "$CASE"' EXIT
: > "$OUT"

say() { printf '%s\n' "$*" | tee -a "$OUT"; }

command -v sccache >/dev/null 2>&1 || { say "ERROR: sccache not installed"; exit 2; }

MON_PID=""
monitor_start() {
  : > "$MON"
  (
    peak=0
    while :; do
      s=$(ps -eo rss=,comm= 2>/dev/null \
          | awk '$2 ~ /^(cargo|rustc|cc|clang|clang-[0-9.]+|collect2|ld\.lld|lld)$/ {t+=$1} END {print t+0}')
      if [ "${s:-0}" -gt "$peak" ]; then peak=$s; printf '%s' "$peak" > "$MON"; fi
      sleep 0.2
    done
  ) >/dev/null 2>&1 &
  MON_PID=$!
}
monitor_stop() { [ -n "$MON_PID" ] && { kill "$MON_PID" 2>/dev/null; wait "$MON_PID" 2>/dev/null; MON_PID=""; }; }

run_case() {
  label="$1"; shift
  say ""
  say "===== $label ====="
  say "  RUSTC_WRAPPER=${RUSTC_WRAPPER:-<unset>}  CARGO_INCREMENTAL=${CARGO_INCREMENTAL:-<unset>}"
  monitor_start
  t0=$(date +%s%N)
  "$@" >"$CASE" 2>&1
  rc=$?
  t1=$(date +%s%N)
  monitor_stop
  ms=$(( (t1 - t0) / 1000000 ))
  peak=$(cat "$MON" 2>/dev/null || echo 0)
  secs=$(awk -v ms="$ms" 'BEGIN{printf "%.1f", ms/1000}')
  say "  exit=$rc  wall=${secs}s  peakRSS=$(( ${peak:-0} / 1024 )) MiB (sampled)"
  printf '%-46s exit=%-2s wall_ms=%-9s peakRSS_MiB=%s\n' "$label" "$rc" "$ms" "$(( ${peak:-0} / 1024 ))" >> "$OUT"
  [ "$rc" -ne 0 ] && tail -5 "$CASE" | sed 's/^/    /'
  return $rc
}

say "sccache A/B/C/D — $(date -u +%FT%TZ)"
say "HEAD:      $(git log --oneline -1)"
say "sccache:   $(sccache --version 2>/dev/null)"
say "rustc:     $(rustc --version)"
say "profile:   $(grep -A6 '^\[profile.dev\]' Cargo.toml | grep -E '^(debug|split-debuginfo)' | tr '\n' ' ')"

export SCCACHE_DIR="$HOME/.cache/sccache"
export SCCACHE_CACHE_SIZE=10G

# ---- A: baseline, no cache -------------------------------------------------
unset RUSTC_WRAPPER
export CARGO_TARGET_DIR="$PWD/target"
$SER cargo clean >/dev/null 2>&1
run_case "A no-sccache cold" $SER cargo test --workspace --no-run

# ---- B: sccache, cold (every crate misses) ---------------------------------
export RUSTC_WRAPPER=sccache
sccache --stop-server >/dev/null 2>&1
$SER cargo clean >/dev/null 2>&1
sccache --zero-stats >/dev/null 2>&1
run_case "B sccache cold (misses)" $SER cargo test --workspace --no-run
say ""
sccache --show-stats 2>/dev/null | sed 's/^/  /' | tee -a "$OUT"

# ---- C: sccache, clean target, warm cache ----------------------------------
$SER cargo clean >/dev/null 2>&1
sccache --zero-stats >/dev/null 2>&1
run_case "C sccache clean+warm (hits)" $SER cargo test --workspace --no-run
say ""
sccache --show-stats 2>/dev/null | sed 's/^/  /' | tee -a "$OUT"

# ---- D: the fleet case — a second, empty target dir -----------------------
export CARGO_TARGET_DIR="$PWD/target-scc-probe"
$SER cargo clean >/dev/null 2>&1
sccache --zero-stats >/dev/null 2>&1
run_case "D fresh target dir, warm cache" $SER cargo test --workspace --no-run
say ""
sccache --show-stats 2>/dev/null | sed 's/^/  /' | tee -a "$OUT"

# ---- verification: does debug info survive a cache HIT? --------------------
# The risk this guards: split-debuginfo="unpacked" emits .dwo sidecars, and if
# sccache restores the binary but not the sidecars, debug info breaks silently
# and only on hits.
say ""
say "===== split-debuginfo × sccache ====="
BIN=$(find target/debug/deps -maxdepth 1 -type f -name 'dedup_unreadable_shard-*' ! -name '*.d' -perm -u+x 2>/dev/null | head -1)
if [ -n "$BIN" ]; then
  say "  test binary:            $(basename "$BIN")"
  say "  .debug_* sections:      $(readelf -S "$BIN" 2>/dev/null | grep -c '\.debug_')"
  say "  .dwo sidecars in deps:  $(find target/debug/deps -maxdepth 2 -name '*.dwo' 2>/dev/null | wc -l)"
  if readelf --debug-dump=info "$BIN" >/dev/null 2>&1; then
    say "  readelf --debug-dump=info: parses cleanly"
  else
    say "  readelf --debug-dump=info: FAILED to parse"
  fi
else
  say "  WARNING: could not locate the test binary to inspect"
fi

# Direct probe: a panic under the same flags, compiled twice so the second is a hit.
P=.tmp/scratch/sdi-probe
mkdir -p "$P"
printf 'fn main(){ panic!("sdi-probe"); }\n' > "$P/p.rs"
sccache rustc -C debuginfo=1 -C split-debuginfo=unpacked -o "$P/p1" "$P/p.rs" >/dev/null 2>&1
rm -f "$P"/p1*
sccache rustc -C debuginfo=1 -C split-debuginfo=unpacked -o "$P/p2" "$P/p.rs" >/dev/null 2>&1
lines=$("$P/p2" 2>&1 | grep -c 'p\.rs:1' || true)
say "  panic resolves file:line on a cache hit: $lines (expect >=1)"
say "  dwo emitted beside probe: $(find "$P" -name '*.dwo' 2>/dev/null | wc -l)"

say ""
say "===== verdict inputs ====="
say "  A and C are the comparison that decides adoption."
say "  Inverted world: if C is not materially faster than A, do not adopt."
say "  target-scc-probe/ left in place for inspection; remove it after."

#!/usr/bin/env bash
# Round 2 — the fleet case, with the two settings round 1 lacked.
#
# Round 1 finding that motivates this: case D (fresh target dir, warm cache) got
# 0.00% Rust hits while C/C++ hit 99.68% and assembler 100%. Reason candidates:
#   - SCCACHE_BASEDIRS was "(none)", so absolute paths (output dir, crate roots)
#     are part of the cache key → a different target dir is a different key.
#   - 229 calls were non-cacheable for "incremental", so dev-profile incremental
#     compilation is excluded from caching entirely.
# This round sets both and re-tests the fresh-target-dir case.
#
# E1 populates under the new key; E2 is the measurement (clean target, warm cache).
set -u
cd /home/vaz/Code/Personal/Rust/opencode-agents-memory || exit 1

SER="sh /home/vaz/Code/Personal/Rust/opencode-agents-memory/.tmp/scripts/cargo-serialized.sh"
OUT=.tmp/scratch/sccache-ef.txt
mkdir -p .tmp/scratch
MON=$(mktemp .tmp/scratch/efmon.XXXXXX); CASE=$(mktemp .tmp/scratch/efcase.XXXXXX)
trap 'rm -f "$MON" "$CASE"' EXIT
: > "$OUT"
say() { printf '%s\n' "$*" | tee -a "$OUT"; }

monitor_start() { : > "$MON"; ( peak=0; while :; do s=$(ps -eo rss=,comm= 2>/dev/null | awk '$2 ~ /^(cargo|rustc|cc|clang|clang-[0-9.]+|collect2|ld\.lld|lld)$/ {t+=$1} END {print t+0}'); if [ "${s:-0}" -gt "$peak" ]; then peak=$s; printf '%s' "$peak" > "$MON"; fi; sleep 0.2; done ) >/dev/null 2>&1 & MON_PID=$!; }
monitor_stop() { [ -n "${MON_PID:-}" ] && { kill "$MON_PID" 2>/dev/null; wait "$MON_PID" 2>/dev/null; MON_PID=""; }; }

run_case() {
  label="$1"; shift
  say ""; say "===== $label ====="
  monitor_start
  t0=$(date +%s%N); "$@" >"$CASE" 2>&1; rc=$?; t1=$(date +%s%N)
  monitor_stop
  ms=$(( (t1 - t0) / 1000000 )); peak=$(cat "$MON" 2>/dev/null || echo 0)
  say "  exit=$rc  wall=$(awk -v ms="$ms" 'BEGIN{printf "%.1f", ms/1000}')s  peakRSS=$(( ${peak:-0} / 1024 )) MiB (sampled)"
  printf '%-44s exit=%-2s wall_ms=%-9s peakRSS_MiB=%s\n' "$label" "$rc" "$ms" "$(( ${peak:-0} / 1024 ))" >> "$OUT"
  [ "$rc" -ne 0 ] && tail -5 "$CASE" | sed 's/^/    /'
  return $rc
}

say "round 2 — BASEDIRS + no incremental, fresh target dir — $(date -u +%FT%TZ)"
say "HEAD: $(git log --oneline -1)"

export SCCACHE_DIR="$HOME/.cache/sccache"
export SCCACHE_CACHE_SIZE=10G
export SCCACHE_BASEDIRS=/home/vaz/Code/Personal/Rust
export CARGO_INCREMENTAL=0
export RUSTC_WRAPPER=sccache

# The server must be restarted so it inherits BASEDIRS: the client spawns it.
sccache --stop-server >/dev/null 2>&1
sleep 1

# ---- E1: populate under the BASEDIRS key -----------------------------------
export CARGO_TARGET_DIR="$PWD/target-scc-e"
$SER cargo clean >/dev/null 2>&1
sccache --zero-stats >/dev/null 2>&1
run_case "E1 populate (BASEDIRS, no incremental)" $SER cargo test --workspace --no-run
say ""
sccache --show-stats 2>/dev/null | grep -E 'Cache hits|Cache misses |Base directories|Non-cacheable reasons|incremental|Cache size' | sed 's/^/  /' | tee -a "$OUT"

# ---- E2: the measurement — clean target, warm cache -----------------------
$SER cargo clean >/dev/null 2>&1
sccache --zero-stats >/dev/null 2>&1
run_case "E2 fresh target dir, warm (THE fleet case)" $SER cargo test --workspace --no-run
say ""
sccache --show-stats 2>/dev/null | tee -a "$OUT" | grep -E 'Compile requests|Cache hits|Cache misses |Cache hits rate|Base directories|Non-cacheable reasons|incremental|crate-type|Compilation failures|Cache size' | sed 's/^/  /'

say ""
say "===== round-2 verdict inputs ====="
say "  Round 1 D (fresh target dir, no BASEDIRS): 388.4s, Rust hit rate 0.00%"
say "  E2 is the same case with BASEDIRS + CARGO_INCREMENTAL=0."
say "  Inverted world: if E2's Rust hit rate is still ~0%, BASEDIRS does not"
say "  normalise what differs between target dirs and the fleet case stays a miss."

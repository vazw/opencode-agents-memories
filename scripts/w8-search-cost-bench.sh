#!/usr/bin/env bash
# W8 search-cost benchmark driver (issue 43 §43.10).
#
# Builds the measurement binary once, then per cell: seed -> cold series (one
# fresh process per sample) -> warm series (one process) -> aggregate -> delete
# the cell root. Every command and exit code lands in raw/driver.log.
set -euo pipefail

WT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ROOT=$(cd "$WT/../../.." && pwd)
SER="$ROOT/.tmp/scripts/cargo-serialized.sh"
SCRATCH="$ROOT/.tmp/scratch/w8"
TMPDIR_RUN="$ROOT/.tmp/scratch"
EVID="$WT/docs/evidence/issue-43-w8-benchmark-20261030"
OUT="${W8_OUT:-$EVID/raw}"
LOG="$OUT/driver.log"
BASELINE="$OUT/baseline.tsv"
STAMP="${W8_STAMP:-$(date -u +%FT%TZ)}"
ROW_SEED="${W8_ROW_SEED:-16909060}"
QUERY_SEED="${W8_QUERY_SEED:-1428696960}"
DEFAULT_REPS="${W8_REPS:-20}"
DEFAULT_COLD="${W8_COLD_SAMPLES:-20}"

SMOKE=0
CELLS_ARG=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --smoke) SMOKE=1 ;;
    --cells)
      shift
      CELLS_ARG="${1:-}"
      ;;
    --cells=*) CELLS_ARG="${1#--cells=}" ;;
    *)
      echo "usage: $0 [--smoke] [--cells name[,name...]]" >&2
      exit 2
      ;;
  esac
  shift
done

mkdir -p "$OUT" "$SCRATCH"

log() {
  printf '%s %s\n' "$(date -u +%FT%TZ)" "$*" >>"$LOG"
}

run() {
  local rc=0
  log "EXEC $*"
  "$@" >>"$LOG" 2>&1 || rc=$?
  log "EXIT $rc"
  return "$rc"
}

CELL_ORDER=(
  x1-inline-cap50k
  x10-inline-cap50k
  x10-inline-cap12k5
  x10-narrow-cap50k
  x10-narrow-cap12k5
  x100-inline-cap50k
  x100-inline-cap12k5
  x100-narrow-cap50k
  x100-narrow-cap12k5
)

# multiplier cap layout
cell_spec() {
  case "$1" in
    x1-inline-cap50k) echo "1 50000 inline" ;;
    x10-inline-cap50k) echo "10 50000 inline" ;;
    x10-inline-cap12k5) echo "10 12500 inline" ;;
    x10-narrow-cap50k) echo "10 50000 narrow" ;;
    x10-narrow-cap12k5) echo "10 12500 narrow" ;;
    x100-inline-cap50k) echo "100 50000 inline" ;;
    x100-inline-cap12k5) echo "100 12500 inline" ;;
    x100-narrow-cap50k) echo "100 50000 narrow" ;;
    x100-narrow-cap12k5) echo "100 12500 narrow" ;;
    *) return 1 ;;
  esac
}

wait_for_quiet() {
  local loadavg1
  local attempt
  for attempt in $(seq 1 240); do
    loadavg1=$(cut -d' ' -f1 /proc/loadavg)
    if awk -v loadavg1="$loadavg1" 'BEGIN { exit !(loadavg1 < 2) }'; then
      return 0
    fi
    log "loadavg $loadavg1 >= 2, waiting before cell start (attempt $attempt)"
    sleep 5
  done
  log "loadavg stayed >= 2; refusing to measure contaminated numbers"
  return 1
}

check_disk() {
  local multiplier="$1"
  if [ "$multiplier" -lt 100 ]; then
    return 0
  fi
  local available
  available=$(df --output=avail -k "$SCRATCH" | tail -n 1 | tr -d ' ')
  if [ -z "$available" ] || [ "$available" -lt 4194304 ]; then
    log "only ${available:-0} KiB available under $SCRATCH; a 100x cell needs >= 4 GiB"
    return 1
  fi
  return 0
}

measure_entry() {
  local entry="$1"
  shift
  run env \
    W8_CELL="$CELL_NAME" \
    W8_ROOT="$CELL_ROOT" \
    W8_ROWS="$CELL_ROWS" \
    W8_CAP="$CELL_CAP" \
    W8_LAYOUT="$CELL_LAYOUT" \
    W8_OUT="$OUT" \
    W8_ROW_SEED="$ROW_SEED" \
    W8_QUERY_SEED="$QUERY_SEED" \
    W8_STAMP="$STAMP" \
    W8_REPS="$CELL_REPS" \
    TMPDIR="$TMPDIR_RUN" \
    "$BIN" --ignored --exact "$entry" --test-threads=1
}

seed_cell() {
  measure_entry w8_seed_cell
}

cold_series() {
  local sample
  for ((sample = 1; sample <= CELL_COLD; sample++)); do
    sync
    measure_entry w8_sample_cold
  done
}

verify_registry() {
  local row
  row=$(sqlite3 "$CELL_ROOT/metadata.db" \
    "select count(*), coalesce(sum(vector_count),0), coalesce(min(vector_count),0) from shards;" 2>>"$LOG") || {
    log "sqlite3 registry read failed for $CELL_NAME"
    return 1
  }
  IFS='|' read -r actual_shards actual_sum actual_min <<<"$row"
  local expected
  expected=$(((CELL_ROWS + CELL_CAP - 1) / CELL_CAP))
  if [ "$expected" -lt 1 ]; then
    expected=1
  fi
  if [ "$actual_shards" != "$expected" ] || [ "$actual_sum" != "$CELL_ROWS" ] || [ "$actual_min" -le 0 ]; then
    log "registry mismatch for $CELL_NAME: shards=$actual_shards (expected $expected) sum=$actual_sum (rows $CELL_ROWS) min=$actual_min"
    return 1
  fi
  log "registry ok for $CELL_NAME: shards=$actual_shards sum=$actual_sum min=$actual_min"
  return 0
}

cleanup_cell() {
  if [ -e "$CELL_ROOT" ]; then
    find "$CELL_ROOT" -depth -delete || {
      log "cleanup failed for $CELL_ROOT (recorded, not unwrapped)"
      return 1
    }
  fi
  return 0
}

run_cell() {
  CELL_NAME="$1"
  CELL_ROWS="$2"
  CELL_CAP="$3"
  CELL_LAYOUT="$4"
  CELL_COLD="$5"
  CELL_REPS="$6"
  local multiplier="${7:-1}"
  CELL_ROOT="$SCRATCH/$CELL_NAME"
  local jsonl="$OUT/$CELL_NAME.jsonl"
  local summary="$OUT/$CELL_NAME.summary.json"

  if [ -e "$summary" ]; then
    log "resume: $CELL_NAME already has a summary, skipping"
    return 0
  fi
  if ! check_disk "$multiplier"; then
    return 1
  fi
  if ! wait_for_quiet; then
    return 1
  fi
  if [ -e "$CELL_ROOT" ]; then
    log "removing stale cell root $CELL_ROOT"
    if ! cleanup_cell; then
      return 1
    fi
  fi
  : >"$jsonl"
  log "cell $CELL_NAME start rows=$CELL_ROWS cap=$CELL_CAP layout=$CELL_LAYOUT cold=$CELL_COLD reps=$CELL_REPS"

  seed_cell || {
    log "seed failed for $CELL_NAME"
    return 1
  }
  if ! verify_registry; then
    return 1
  fi

  cold_series || {
    log "cold series failed for $CELL_NAME"
    return 1
  }
  if grep -q '"cold_verified":false' "$jsonl"; then
    log "cold verification failed for $CELL_NAME; re-running the cell once"
    cleanup_cell || return 1
    : >"$jsonl"
    seed_cell || {
      log "re-seed failed for $CELL_NAME"
      return 1
    }
    verify_registry || return 1
    cold_series || {
      log "cold re-run failed for $CELL_NAME"
      return 1
    }
    if grep -q '"cold_verified":false' "$jsonl"; then
      log "cold verification failed twice for $CELL_NAME; failing the step"
      return 1
    fi
  fi

  measure_entry w8_warm_series || {
    log "warm series failed for $CELL_NAME"
    return 1
  }
  measure_entry w8_aggregate || {
    log "aggregate failed for $CELL_NAME"
    return 1
  }
  if [ ! -s "$summary" ]; then
    log "summary missing after aggregate for $CELL_NAME"
    return 1
  fi
  if ! cleanup_cell; then
    return 1
  fi
  log "cell $CELL_NAME done"
  return 0
}

# ---------------------------------------------------------------------------
# Preflight and build
# ---------------------------------------------------------------------------

for tool in sqlite3 find sync df awk cut tail seq tr grep; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    log "preflight: missing required tool $tool"
    exit 1
  fi
done
if [ ! -f "$BASELINE" ]; then
  log "preflight: missing $BASELINE (Step 0 must run first)"
  exit 1
fi
B=$(awk -F'\t' 'NR==1 { print $3 }' "$BASELINE")
if [ -z "$B" ] || [ "$B" -le 0 ]; then
  log "preflight: baseline.tsv has no usable row count"
  exit 1
fi
log "W8 START stamp=$STAMP baseline_B=$B out=$OUT"

BUILD_OUTPUT=$(sh "$SER" cargo test --release -p mem-store --test search_cost_bench --no-run 2>&1) || {
  printf '%s\n' "$BUILD_OUTPUT" >>"$LOG"
  log "release build failed"
  exit 1
}
printf '%s\n' "$BUILD_OUTPUT" >>"$LOG"
BIN=$(printf '%s\n' "$BUILD_OUTPUT" | sed -n 's/.*Executable tests\/search_cost_bench.rs (\(.*\))/\1/p' | tail -n 1)
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
  log "could not locate the release measurement binary"
  exit 1
fi
log "measurement binary: $BIN"

# ---------------------------------------------------------------------------
# Cells
# ---------------------------------------------------------------------------

if [ "$SMOKE" -eq 1 ]; then
  run_cell smoke 1000 12500 inline 3 3 1 || exit 1
else
  if [ -n "$CELLS_ARG" ]; then
    IFS=',' read -r -a requested <<<"$CELLS_ARG"
    for name in "${requested[@]}"; do
      spec=$(cell_spec "$name") || {
        log "unknown cell name: $name"
        exit 1
      }
      read -r multiplier cap layout <<<"$spec"
      rows=$((multiplier * B))
      run_cell "$name" "$rows" "$cap" "$layout" "$DEFAULT_COLD" "$DEFAULT_REPS" "$multiplier" || exit 1
    done
  else
    for name in "${CELL_ORDER[@]}"; do
      read -r multiplier cap layout <<<"$(cell_spec "$name")"
      rows=$((multiplier * B))
      run_cell "$name" "$rows" "$cap" "$layout" "$DEFAULT_COLD" "$DEFAULT_REPS" "$multiplier" || exit 1
    done
  fi
fi

log "W8 END stamp=$(date -u +%FT%TZ)"
echo "W8 DONE: $(date -u +%FT%TZ) (log: $LOG)"

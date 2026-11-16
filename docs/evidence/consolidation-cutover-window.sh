#!/usr/bin/env bash
# Cutover window driver — DRAFT. Running this without a recorded Step 11 approval
# is a production mutation. See docs/evidence/consolidation-cutover-runbook.md.
#
#   run-window.sh status    read-only state dump
#   run-window.sh stop      stop the host via `opencode service stop`, verify no holder
#   run-window.sh restart   `opencode service restart`
#   run-window.sh window    install, acquire, snapshot, plan, dry-run, apply,
#                           staged-verify, install, settlement, release
#   run-window.sh all       stop + window
#
# Launch from a terminal that is NOT an opencode session, or detached — a restart
# kills every opencode session, including the one that started it:
#   setsid -f ./run-window.sh all </dev/null >>.tmp/scratch/step11/window.log 2>&1
#
# Every path is overridable, so the whole driver can be rehearsed off-production:
#   S=<scratch>/data P=<scratch> C=<scratch>/custody E=<scratch>/engine \
#   ARTIFACT=... EXPECTED_SHA256=... ./run-window.sh window
#
# Never deletes anything. Any non-zero step aborts the run with custody held.

set -euo pipefail
umask 077

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
S=${S:-/home/opencode/.opencode-mem/data}
P=${P:-/home/opencode/.opencode-mem}
# C is DERIVED after acquire, never set here: release resolves custody by scanning
# <parent>/recovery for a directory whose name ends in -<wtok> and IGNORES
# --custody (src/shard_consolidation.rs:4577-4600). A custody dir at any other
# path is invisible to release and the window ends in 77 release_phase_nonterminal.
C=
R=$P/.opencode-mem-maintenance.lock
OPENCODE_BIN=${OPENCODE_BIN:-opencode}
ARTIFACT=${ARTIFACT:-$ROOT/.tmp/cutover-artifact/opencode-agents-memory}
EXPECTED_SHA256=${EXPECTED_SHA256:-}
E=${E:-$ROOT/plugin/engine/linux-x64/opencode-agents-memory}

say() { printf '\n== %s\n' "$*"; }
die() { printf 'ABORT: %s\n' "$*" >&2; exit 1; }

code_name() {
  case $1 in
    64) echo 'EXIT_PLAN_INVALID' ;; 65) echo 'EXIT_UNSUPPORTED_SHAPE' ;;
    71) echo 'EXIT_DIVERGENT_ROWS' ;; 72) echo 'EXIT_IDENTITY_REFUSAL' ;;
    73) echo 'EXIT_EMBEDDING_REFUSAL' ;; 74) echo 'EXIT_STORE_OR_IO' ;;
    75) echo 'ENGINE_LOCK_LOST' ;; 76) echo 'held (foreign holder)' ;;
    77) echo 'EXIT_CUSTODY_REFUSAL' ;; 78) echo 'MCP unimplemented' ;;
    *) echo 'unknown' ;;
  esac
}

run() {
  local name=$1; shift
  say "$name"
  printf '+'; printf ' %q' "$@"; printf '\n'
  local rc=0
  "$@" || rc=$?
  if [ "$rc" -ne 0 ]; then
    printf 'step failed: %s exit=%d (%s)\n' "$name" "$rc" "$(code_name "$rc")" >&2
    status
    # `${X:-}` guards: this message must survive a failure BEFORE acquire, when
    # NONCE and C are not set yet and `set -u` would otherwise kill the handler.
    die "$name refused. State:
  reservation: $R
  custody:     ${C:-<not created yet>}
  nonce:       ${NONCE:-<not minted yet>}
  recover (only if custody exists):
  OPENCODE_MEM_MAINTENANCE_NONCE=${NONCE:-<nonce>} $E --mode=consolidate-recover --storage-root $S --custody ${C:-<custody>}
  never hand-delete $R"
  fi
}

# Captures stdout (stderr still reaches the terminal) into CAPTURED.
run_capture() {
  local name=$1; shift
  say "$name"
  printf '+'; printf ' %q' "$@"; printf '\n'
  local rc=0 out
  out=$("$@") || rc=$?
  [ -n "$out" ] && printf '%s\n' "$out"
  if [ "$rc" -ne 0 ]; then
    printf 'step failed: %s exit=%d (%s)\n' "$name" "$rc" "$(code_name "$rc")" >&2
    status
    die "$name refused; custody held — use --mode=consolidate-recover, never hand-delete $R"
  fi
  CAPTURED=$out
}

# pids of processes whose argv[0] is the engine binary. Anchored to argv[0] on
# purpose: a bare `pgrep -f opencode-agents-memory` matches every shell whose
# command line mentions this repo path — including the driver itself and any
# `bash -c cd <repo> && …` — and would abort a clean window.
engine_pids() {
  pgrep -f '^([^ ]*/)?opencode-agents-memory( |$)' || true
}

# The last journal line that is a real phase: bookkeeping records carry a phase
# name but are not phases (`effective_phase` in src/shard_consolidation.rs skips
# them). `TailDiscarded` is listed because the design names it; no writer exists.
effective_journal_line() {
  jq -c 'select(.phase != "Retaken" and .phase != "TailDiscarded")' "$C/journal.json" | tail -n1
}

# Human-readable companion to fd_holders_under_s: pid, comm and the first fd
# target under S, for the abort dump. The boolean decision uses
# fd_holders_under_s; this only names the culprit.
holders_detail() {
  local p f t
  for p in /proc/[0-9]*; do
    [ -d "$p/fd" ] || continue
    for f in "$p"/fd/*; do
      t=$(readlink "$f" 2>/dev/null) || continue
      case "$t" in
        "$S"/*) printf '  %s %s %s\n' "${p#/proc/}" "$(cat "$p/comm" 2>/dev/null)" "$t"; break ;;
      esac
    done
  done
}

status() {
  say 'state'
  printf 'S=%s\nP=%s\nC=%s\nR=%s\n' "$S" "$P" "$C" "$R"
  printf 'reservation: '; [ -e "$R" ] && jq -c . "$R" 2>/dev/null || echo absent
  # The journal is JSONL, one record per line — `.records[-1]` is wrong and would
  # print every phase. tail the last line instead.
  # The journal is JSONL and bookkeeping records (`Retaken`, `TailDiscarded`) are
  # never phases — `effective_phase` skips them. Mirror that here or the status
  # dump reports a crash-mid-retake window as "Retaken" when it is not.
  printf 'journal:     '; [ -n "$C" ] && [ -f "$C/journal.json" ] \
    && effective_journal_line | jq -c '{seq,phase}' 2>/dev/null || echo absent
  printf 'engine lock: '; cat "$S/.engine.lock" 2>/dev/null && echo || echo absent
  printf 'engine procs: '; engine_pids | grep -c . || true
  printf 'fd holders under S:\n'
  local holders; holders=$(fd_holders_under_s)
  if [ -n "$holders" ]; then holders_detail; else echo '  none'; fi
  # Unreadable fd dirs refuse acquire (fail-closed) — except sshd sessions, which
  # H3 exempts. Report both classes separately so the dump matches the engine.
  local uid p bad='' sshd=0
  uid=$(id -u)
  for p in /proc/[0-9]*; do
    grep -q "^Uid:.*[[:space:]]${uid}[[:space:]]" "$p/status" 2>/dev/null || continue
    [ -r "$p/fd" ] && continue
    if [ "$(cat "$p/comm" 2>/dev/null)" = 'sshd-session' ]; then
      sshd=$((sshd+1))
    else
      bad="$bad ${p#/proc/}($(cat "$p/comm" 2>/dev/null))"
    fi
  done
  printf 'unreadable fd dirs: sshd-session (exempt in H3)=%s, other (fail-closed)=%s\n' \
    "$sshd" "${bad:-none}"
  return 0
}

# Prints the pids of same-uid processes holding a descriptor under S.
fd_holders_under_s() {
  local p f
  for p in /proc/[0-9]*; do
    [ -d "$p/fd" ] || continue
    for f in "$p"/fd/*; do
      case $(readlink "$f" 2>/dev/null) in
        "$S"/*) printf '%s\n' "${p#/proc/}"; break ;;
      esac
    done
  done
}

preflight() {
  say 'preflight'
  [ -x "$ARTIFACT" ] || die "artifact missing: $ARTIFACT"
  local got; got=$(sha256sum "$ARTIFACT" | cut -d' ' -f1)
  [ -n "$EXPECTED_SHA256" ] || die 'set EXPECTED_SHA256 to the re-bound hash'
  [ "$got" = "$EXPECTED_SHA256" ] || die "artifact hash mismatch: got $got want $EXPECTED_SHA256"
  # Mode-surface probe: a build WITH consolidate-acquire answers 77 on a bad root,
  # a build without it answers 64. This is the staleness discriminator.
  local probe=0
  "$ARTIFACT" --mode=consolidate-acquire --storage-root /nonexistent-probe-root >/dev/null 2>&1 || probe=$?
  [ "$probe" -eq 77 ] || die "artifact is stale: consolidate-acquire probe exit=$probe (want 77)"
  # §0 B2 hard fence, learned from window attempt 2: with the plugin line
  # enabled, any opencode host start (service or TUI) spawns the engine beside
  # it, and one did — mid-window, between verify_stopped and acquire, which is
  # exactly the acquire_writer_active 77. The plugin must stay disabled until
  # §12 re-enables it.
  local cfg=${HOME}/.config/opencode/opencode.jsonc
  if grep -E '^[[:space:]]*"/home/opencode/Code/Personal/Rust/opencode-agents-memory/plugin"' "$cfg" >/dev/null 2>&1; then
    die 'plugin line is ENABLED in ~/.config/opencode/opencode.jsonc — comment it out first (§0 B2), re-enable only after the window'
  fi
  [ -d "$P" ] && [ -d "$S" ] || die 'storage root or parent missing'
  [ ! -e "$R" ] || die "reservation already exists — window already started or not released"
  local free need
  free=$(df -Pk "$P" | awk 'NR==2{print $4}')
  need=$(( $(du -sk "$S" | cut -f1) * 3 ))
  [ "$free" -gt "$need" ] || die "insufficient space: free ${free}K, need ${need}K"
  echo "ok (artifact $got)"
}

do_stop() {
  say 'stop'
  command -v "$OPENCODE_BIN" >/dev/null \
    || die "$OPENCODE_BIN not on PATH — check the symlink /home/opencode/.local/bin/opencode"
  # `opencode service status` exits 0 either way and prints the URL, or the literal
  # word `stopped` (packages/cli/src/commands/handlers/service/status.ts). Read the
  # output; the exit code carries no information.
  local where
  where=$("$OPENCODE_BIN" service status 2>/dev/null || echo stopped)
  if [ "$where" = 'stopped' ]; then
    echo 'service already stopped'
  else
    echo "stopping $where"
    "$OPENCODE_BIN" service stop
  fi
  local waited=0
  while [ "$waited" -lt 30 ] && [ -n "$(engine_pids)" ]; do
    sleep 1; waited=$((waited+1))
  done
  [ -z "$(engine_pids)" ] || die "engine survived the ladder: $(engine_pids | tr '\n' ' ') — inspect before continuing"
  echo 'host and engine down'
}

verify_stopped() {
  say 'verify stopped'
  [ -z "$(engine_pids)" ] || die "engine still running: $(engine_pids | tr '\n' ' ')"
  local holders; holders=$(fd_holders_under_s)
  [ -z "$holders" ] || die "fd holder survives: $(printf '%s' $holders)"
  echo 'no fd holder under S'
}

do_window() {
  preflight
  verify_stopped

  run 'install artifact' install -m 0755 "$ARTIFACT" "$E"

  # The plan generator binds a baseline manifest that must sit beside S ($P or
  # $P/custody); no shipped mode writes one outside a custody snapshot, and the
  # window creates its custody only at the snapshot step BELOW this one. So the
  # baseline is built here, with S stopped and quiescent (verify_stopped above).
  # Format and exclusions match the custody manifest: `sha256  ./relpath`, no
  # `status`, no `.engine.lock`. The plan never verifies it against the tree —
  # its only job is to be a stable digest bound into the plan.
  say 'baseline manifest'
  ( cd "$S" && find . -type f ! -name status ! -name .engine.lock -print0 \
      | LC_ALL=C sort -z | xargs -0 sha256sum ) > "$P/manifest.sha256" \
    || die 'baseline manifest could not be written'
  echo "$(wc -l < "$P/manifest.sha256") entries -> $P/manifest.sha256"

  # Plan and dry-run come BEFORE acquire, and that ordering is not cosmetic.
  # `consolidate-shards --dry-run` is fenced by `refuse_if_reservation_held`
  # (src/main.rs:874) — the dry-run arm has no nonce admission, unlike the apply
  # arm which goes through `admit_window_step`. A dry-run under a held window is
  # therefore refused 76 by design, not by accident. Both steps are read-only,
  # and the host is stopped with the plugin disabled (§0 B2), so nothing writes
  # S here.
  PLAN=$P/plan-$(date -u +%Y%m%dT%H%M%SZ).json
  run 'generate plan' python3 "$ROOT/docs/evidence/consolidation-plan-generator-20261113/build_plan.py" "$S" "$PLAN"
  run 'dry-run' env OPENCODE_MEM_MAINTENANCE_NONCE= "$E" --mode=consolidate-shards --storage-root "$S" --plan "$PLAN" --dry-run

  # Final fence re-check, learned from attempt 2: a respawner put a live engine
  # under S in the seconds between the first verify_stopped and this acquire —
  # the refusal was correct and the driver's scan was simply stale. Re-scan at
  # the last possible moment so the gap is milliseconds, not seconds.
  verify_stopped

  # Attempt 3 lesson: the refusal can name nothing because the holder is gone
  # by the time the failure dump runs. Watch /proc for 15 s around the acquire
  # and print whatever appears, so a transient engine names itself.
  WLOG=$(mktemp "${TMPDIR:-/tmp}/window-holders.XXXXXX")
  ( end=$((SECONDS+15))
    while [ "$SECONDS" -lt "$end" ]; do
      holders_detail >> "$WLOG"
      sleep 0.25
    done ) &
  WATCHER=$!

  say 'acquire'
  printf '+'; printf ' %q' env OPENCODE_MEM_MAINTENANCE_NONCE= "$E" --mode=consolidate-acquire --storage-root "$S"; printf '\n'
  ARC=0
  env OPENCODE_MEM_MAINTENANCE_NONCE= "$E" --mode=consolidate-acquire --storage-root "$S" || ARC=$?
  kill "$WATCHER" 2>/dev/null || true
  wait "$WATCHER" 2>/dev/null || true
  if [ -s "$WLOG" ]; then
    say 'holders observed during the acquire step'
    sort -u "$WLOG" | sed -n '1,20p'
  fi
  rm -f "$WLOG"
  if [ "$ARC" -ne 0 ]; then
    printf 'step failed: acquire exit=%d (%s)\n' "$ARC" "$(code_name "$ARC")" >&2
    status
    die "acquire refused (rc=$ARC) — holders observed during the step are listed above; empty list means the holder lived only inside acquire's own check"
  fi
  [ -f "$R" ] || die 'acquire exited 0 but no reservation file'
  NONCE=$(jq -r .nonce "$R")
  [ -n "$NONCE" ] && [ "$NONCE" != null ] || die 'reservation body has no nonce'
  WT=$(printf '%s' "$NONCE" | sha256sum | cut -c1-16)
  echo "window token <wtok>=$WT"

  # The custody name is part of the release contract, not an operator choice.
  C=$P/recovery/consolidation-$(date -u +%s)-$WT
  [ ! -e "$C" ] || die "custody already exists: $C (snapshot requires a fresh dir)"
  echo "custody=$C"

  run 'snapshot' env OPENCODE_MEM_MAINTENANCE_NONCE="$NONCE" "$E" --mode=consolidate-snapshot --storage-root "$S" --custody "$C"
  run 'verify-snapshot' env OPENCODE_MEM_MAINTENANCE_NONCE="$NONCE" "$E" --mode=consolidate-verify-snapshot --storage-root "$S" --custody "$C"

  run_capture 'apply' env OPENCODE_MEM_MAINTENANCE_NONCE="$NONCE" "$E" --mode=consolidate-shards --storage-root "$S" --plan "$PLAN" --custody "$C"
  # The staged dir name is non-deterministic (plan_sha256|pid|nanos, :1459-1467),
  # so it is read back from the apply report — never guessed.
  STAGED=$(printf '%s' "$CAPTURED" | jq -r '.staged_root // empty')
  [ -n "$STAGED" ] && [ -d "$STAGED" ] || die "apply reported no usable staged_root: $STAGED"
  echo "staged_root=$STAGED"
  run 'staged-verify' env OPENCODE_MEM_MAINTENANCE_NONCE="$NONCE" "$E" --mode=consolidate-staged-verify --storage-root "$S" --custody "$C" --staged "$STAGED"
  run 'install' env OPENCODE_MEM_MAINTENANCE_NONCE="$NONCE" "$E" --mode=consolidate-install --storage-root "$S" --custody "$C" --staged "$STAGED"
  run 'settlement verify' env OPENCODE_MEM_MAINTENANCE_NONCE="$NONCE" "$E" --mode=consolidate-verify --storage-root "$S" --custody "$C"

  say 'journal state'
  effective_journal_line | jq -r '.phase // "unknown"' 2>/dev/null || echo 'unreadable'

  run 'release' env OPENCODE_MEM_MAINTENANCE_NONCE="$NONCE" "$E" --mode=consolidate-release --storage-root "$S"
  # `[ cond ] && die` would exit 1 on SUCCESS under `set -e`; negate instead.
  [ ! -e "$R" ] || die 'release exited 0 but the reservation file is still present'

  say 'window complete — re-enable the plugin, then restart the host'
  echo 'the plugin is still disabled in ~/.config/opencode/opencode.jsonc; uncomment it first,'
  echo 'then: opencode service restart'
  status
}

# Learned from attempts 2 and 3: a host started while the plugin line was
# enabled carries the plugin IN MEMORY — commenting opencode.jsonc does not
# unload it — and spawns engines on demand, exactly in the seconds between a
# scan and the acquire. The window runs only with no opencode process at all.
fence_no_opencode() {
  local strays
  strays=$(pgrep -x opencode || true)
  [ -z "$strays" ] || die "opencode processes still alive: $(printf '%s ' $strays)- quit every opencode session first (including the session showing this output); a live host spawns engines regardless of opencode.jsonc"
}

# An unreadable /proc/<pid>/fd on a same-uid process makes `acquire` fail closed
# (`live_process_holds_fd_under`, crates/mem-store/src/maintenance_reservation.rs)
# — and sshd sets PR_SET_DUMPABLE=0 on its session processes. MEASURED: window
# attempts 2-4 all refused 77 acquire_writer_active against exactly this, while
# every fd scan (which skips unreadable dirs) reported none. Refuse up front and
# name the processes, or the operator sees an unexplainable refusal at acquire.
fence_no_unreadable_fd() {
  # Since H3 (2026-11-16) the engine itself exempts `comm == "sshd-session"` from
  # the unreadable-fd arm, so an open ssh session no longer blocks acquire. What
  # still refuses is any OTHER same-uid process with an unreadable /proc/<pid>/fd
  # — name those, and report the exempted count for the record.
  local uid p bad='' sshd=0
  uid=$(id -u)
  for p in /proc/[0-9]*; do
    grep -q "^Uid:.*[[:space:]]${uid}[[:space:]]" "$p/status" 2>/dev/null || continue
    [ -r "$p/fd" ] && continue
    if [ "$(cat "$p/comm" 2>/dev/null)" = 'sshd-session' ]; then
      sshd=$((sshd+1))
    else
      bad="$bad ${p#/proc/}($(cat "$p/comm" 2>/dev/null))"
    fi
  done
  [ "$sshd" -eq 0 ] || echo "note: $sshd sshd-session process(es) with unreadable fd dirs — exempt in H3, not blocking"
  [ -z "$bad" ] || die "same-uid processes with unreadable /proc/<pid>/fd:$bad - acquire fails closed on these (H3 exempts only sshd-session). Identify them before running: they are neither sshd sessions nor covered by the exemption."
}

case ${1:-} in
  status)  status ;;
  stop)    fence_no_opencode; fence_no_unreadable_fd; do_stop; verify_stopped ;;
  restart) "$OPENCODE_BIN" service restart ;;
  window)  fence_no_opencode; fence_no_unreadable_fd; do_window ;;
  all)     fence_no_opencode; fence_no_unreadable_fd; do_stop; do_window ;;
  *)       sed -n '2,/^$/p' "$0"; exit 2 ;;
esac

#!/usr/bin/env bash
# live-swap.sh — the executable form of docs/runbooks/live-swap.md.
#
# The runbook is the spec; this script is the same sequence with the dangerous
# edges removed: it refuses to write the live store or config while OpenCode
# (or the store's engine) is running, prints every destructive step before it
# happens, backs up first, and never deletes store data (find -delete touches
# only the derived *-shm / *-tshm sidecars; everything else is mv or cp -a).
#
# It never starts OpenCode, never edits the runbook, never runs a git command
# that writes, and never writes the live root unless the mode says so.
#
# Modes (a bare invocation is --dry-run and writes nothing):
#   --dry-run   print the plan and run the read-only preflight. Safe while
#               OpenCode runs.
#   --backup    §3–§5: checkpoint, dated cp -a backup, sha256 manifest,
#               per-DB integrity_check, LATEST pointer, swap.log.
#   --swap      --backup, then §7 (provision the ONNX cache), then §8 (the
#               plugin-entry swap, one quoted token, in place).
#   --verify    §9b/§10: one engine, its parent, store versions, web health,
#               token mint, one search + one add canary, status-file semantics.
#   --rollback  §12: restore both config files, move the port-era store aside,
#               cp -a the backup data back.
#
# usage: live-swap.sh [MODE] [OPTIONS] — see --help.
set -euo pipefail

SCRIPT_PATH=$(readlink -f -- "${BASH_SOURCE[0]}")
REPO_ROOT=$(cd -- "$(dirname -- "$SCRIPT_PATH")/.." && pwd -P)

# ── defaults (§1 measured values; overridable for a rehearsal) ─────────────
STORE_ROOT=$HOME/.opencode-mem
CONFIG_DIR=$HOME/.config/opencode
BACKUP_ROOT=$HOME/.opencode-mem-backups
PLUGIN_DIR=$REPO_ROOT/plugin
ENGINE_NAME=opencode-agents-memory
PROVISIONER=$REPO_ROOT/scripts/provision-onnx-cache.sh

# §7: the tool embedder reads $TMPDIR/.opencode-mem-onnx/<embeddingModel>, and
# the canonical tokenizer is v2.0 — a v1.0 tokenizer answers -32603.
EXPECTED_ONNX_BYTES=547552426
EXPECTED_ONNX_SHA=347440e93b5ec979fdcf6041b72721aade7b9680c16022e2830db7115ff6fd9f

# §7 ran the canary in this project's tier; the tag is a valid
# opencode_<scope>_<hash> container tag (runbook §13 names it).
DEFAULT_CONTAINER_TAG=opencode_project_4182e0858e0c93a1

MODE=dry-run
ASSUME_YES=0
REHEARSE_ROOT=
FROM_TS=
WEB_PORT_OVERRIDE=
CONTAINER_TAG=$DEFAULT_CONTAINER_TAG

FAILS=0
WARNINGS=0
PENDING=0
LOG_FILE=
CONFIRMED=0

# ── small helpers ─────────────────────────────────────────────────────────
now_utc() { date -u +%Y-%m-%dT%H:%M:%SZ; }
utc_stamp() { date -u +%Y%m%dT%H%M%SZ; }

log() {
	local stamp
	stamp=$(now_utc)
	printf '[%s] %s\n' "$stamp" "$*"
	if [ -n "$LOG_FILE" ]; then
		printf '[%s] %s\n' "$stamp" "$*" >>"$LOG_FILE"
	fi
}

die() {
	printf 'live-swap: %s\n' "$*" >&2
	if [ -n "$LOG_FILE" ]; then
		printf '[%s] FATAL %s\n' "$(now_utc)" "$*" >>"$LOG_FILE" 2>/dev/null || true
	fi
	exit 1
}

ck_ok() { printf '  [ ok ] %s\n' "$*"; }
ck_info() { printf '  [info] %s\n' "$*"; }
ck_warn() {
	printf '  [warn] %s\n' "$*"
	WARNINGS=$((WARNINGS + 1))
}
ck_fail() {
	printf '  [FAIL] %s\n' "$*"
	FAILS=$((FAILS + 1))
}
ck_pending() {
	printf '  [pend] %s\n' "$*"
	PENDING=$((PENDING + 1))
}
plan() { printf '  WOULD %s\n' "$*"; }
heading() { printf '\n== %s ==\n' "$*"; }

# confirm() never returns a failure status (a caller must not be able to turn
# `set -e` off through it); it sets CONFIRMED.
confirm() {
	CONFIRMED=0
	if [ "$ASSUME_YES" -eq 1 ]; then
		log "auto-confirmed (--yes): $1"
		CONFIRMED=1
		return 0
	fi
	if [ ! -t 0 ]; then
		die "stdin is not a terminal — re-run with --yes if you mean it: $1"
	fi
	printf 'ABOUT TO: %s\nproceed? [y/N] ' "$1"
	local answer=''
	read -r answer || answer=
	case $answer in
	y | Y | yes | YES) CONFIRMED=1 ;;
	*) CONFIRMED=0 ;;
	esac
	return 0
}

abspath() { realpath -m -- "$1"; }

# valid_restore_stamp <value> — exactly a dated backup directory name: the UTC
# stamp --backup writes, plus an optional -N collision suffix. This is also the
# whole of the --from path validation: no `/`, no `..`, no absolute path.
valid_restore_stamp() {
	[[ $1 =~ ^[0-9]{8}T[0-9]{6}Z(-[0-9]+)?$ ]]
}

is_under() {
	# $1 child, $2 parent — equal counts as under.
	case $1/ in
	"$2"/*) return 0 ;;
	*) return 1 ;;
	esac
}

first_line() {
	local out=''
	out=$(grep -m1 -oE "$1" "$2" 2>/dev/null || true)
	if [ -z "$out" ]; then
		return 0
	fi
	printf '%s\n' "$out"
}

# config_value <file> <key> — the first string value of a JSONC key, unquoted.
config_value() {
	local raw=''
	raw=$(grep -m1 -oE "\"$2\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" "$1" 2>/dev/null || true)
	if [ -z "$raw" ]; then
		return 0
	fi
	printf '%s' "$raw" | sed -E 's/.*:[[:space:]]*"(.*)"$/\1/'
}

# config_number <file> <key> — the first numeric value of a JSONC key.
config_number() {
	local raw=''
	raw=$(grep -m1 -oE "\"$2\"[[:space:]]*:[[:space:]]*[0-9]+" "$1" 2>/dev/null || true)
	if [ -z "$raw" ]; then
		return 0
	fi
	printf '%s' "$raw" | sed -E 's/.*:[[:space:]]*([0-9]+)$/\1/'
}

# ── --help ────────────────────────────────────────────────────────────────
usage() {
	cat <<'HELP'
live-swap.sh — execute docs/runbooks/live-swap.md

  The runbook is the spec. This script is the same sequence, ordered the same
  way, with the safety rails the runbook states as prose made mechanical.

USAGE
  live-swap.sh [MODE] [OPTIONS]

  A bare invocation (no MODE) is --dry-run: it prints the plan and runs the
  read-only preflight, and it writes nothing — safe while OpenCode runs.

  Linux only: the runbook's host is Void Linux, and the script uses GNU
  coreutils (stat -c, df --output, du -sb, find -delete, sort -z).

MODES
  --dry-run   Preflight only. Checks the store, the free space (need ~2x),
              the config values, the plugin dist/engine, whether OpenCode or
              the engine is running, the backup root, and the ONNX cache
              state; prints every command the real modes would run.
              Changes nothing. Exit 3 if a FAIL line appears.

  --backup    Runbook §3-§5, the pre-start half:
                1. refuse if OpenCode or the store's engine is running (§2),
                2. PRAGMA wal_checkpoint(TRUNCATE) on every *.db (expect
                   0|0|0), then find -delete only the derived *-shm/*-tshm;
                   a non-empty *-wal is a refusal, never a delete,
                3. dated cp -a of the store root to
                   ~/.opencode-mem-backups/<UTC> plus both config files,
                4. sha256 MANIFEST.sha256 and `sha256sum -c`,
                5. PRAGMA integrity_check per DB (expect every one `ok`),
                6. the LATEST pointer and a swap.log line per action.
              Re-running makes a new dated backup; nothing is overwritten.

  --swap      --backup, then the runbook's §7-§8:
                1. scripts/provision-onnx-cache.sh (§7; ~38 s cold),
                2. a copy of opencode.jsonc as .pre-swap-<TS>,
                3. the single quoted token "opencode-mem@latest" replaced in
                   place by the absolute plugin directory — one string, same
                   commas, comments and layout untouched,
                4. the diff, and the exact restore command.
              Interactive confirmation before the config edit; --yes for a
              non-interactive run. The script does NOT start OpenCode: it
              prints "start OpenCode now, then run --verify". The store
              migration happens at the next start (§6).

  --verify    After you start OpenCode (runbook §9b/§10), read-only except for
              one canary memory (§12 of the battery):
                1. the engine process count for the store (expect 1),
                2. that engine's parent — must be the OpenCode host (the
                   one-process discriminator; if it is not, stop and record),
                3. the store versions: metadata 2, prompts 2, shards 3 lazily,
                4. web health on the engine's actual port (the engine's own
                   --web-port when its cmdline names one; otherwise
                   --web-port, then OPENCODE_MEM_WEB_PORT, then 3742),
                5. the token at <store>/data/.auth-token, minted by the 401,
                6. one search, one add canary, one search for the canary,
                7. the status file: absent, or naming a dead pid.
              Then prints the MANUAL two-workspace `memory list-shards`
              instruction — the live discriminator this script cannot run.
              Exit 3 on any FAIL. Re-running is safe (one more canary each
              time; nothing else is written).

  --rollback  Runbook §12, the rehearsed move-aside:
                1. refuse if OpenCode or the engine is running,
                2. restore both config files from the latest backup,
                3. mv the port-era store aside to
                   <backup-root>/data.port-<TS> (never deleted),
                4. cp -a the backup's data/ back, then diff -r to prove the
                   copy is byte-identical,
                5. the restart instruction.
              Interactive confirmation; --yes for a non-interactive run.

OPTIONS
  --yes                Do not ask; the owner has already read the plan.
  --rehearse DIR       Operate on a scratch copy, never the live root:
                       DIR/opencode-mem (a store copy), DIR/config (fake
                       config files), DIR/backups. Fails closed if DIR
                       resolves to (or contains) the live store, config or
                       backup root. The live-process refusal is skipped
                       because nothing live is written; a process holding
                       the *scratch* storage root is still a refusal.
  --store DIR          Store root (default ~/.opencode-mem; data/ lives in it).
  --config-dir DIR     Config directory (default ~/.config/opencode).
  --backup-root DIR    Backup root (default ~/.opencode-mem-backups). Must be
                       outside the store root.
  --plugin-dir DIR     Plugin package (default <repo>/plugin).
  --container-tag TAG  Canary container tag for --verify (default: this
                       project's, opencode_project_4182e0858e0c93a1).
  --web-port N         Fallback web port for --verify. The engine's own
                       --web-port on its command line always wins; this flag
                       applies only when that command line names none, then
                       OPENCODE_MEM_WEB_PORT, then 3742. The config's
                       webServerPort 4747 is NOT forwarded by the launch.
  --from TS            --rollback: restore this backup instead of LATEST.
                       TS must be a dated stamp (20261030T175230Z), optional
                       -N collision suffix; `/`, `..` and paths are rejected.
  -h, --help           This text.

SEQUENCE (the runbook's order)
  1. quit OpenCode              this script refuses to write while it runs
  2. live-swap.sh --dry-run     read the plan and the checks
  3. live-swap.sh --backup      checkpoint + dated backup + manifest + integrity
  4. live-swap.sh --swap        provision the cache, swap the plugin entry
  5. start OpenCode             the engine migrates the store in place (§6)
  6. quit, start once more      the fresh-engine reopen (§9)
  7. live-swap.sh --verify      the battery, then the two-workspace check
  8. one week of dogfood        (§11)
  9. live-swap.sh --rollback    only if §11's rollback triggers fire

EXIT STATUS
  0  the mode did what it reports
  1  refused or aborted (nothing live was written, or the write was reverted)
  2  a command-line argument was rejected (unknown flag, or a malformed --from)
  3  the mode ran and a FAIL line was reported (see the output)
  130 the owner answered no
HELP
}

# ── argument parsing ──────────────────────────────────────────────────────
parse_args() {
	while [ $# -gt 0 ]; do
		case $1 in
		--dry-run) MODE=dry-run ;;
		--backup) MODE=backup ;;
		--swap) MODE=swap ;;
		--verify) MODE=verify ;;
		--rollback) MODE=rollback ;;
		--yes) ASSUME_YES=1 ;;
		--rehearse)
			REHEARSE_ROOT=${2:?--rehearse needs a directory}
			shift
			;;
		--store)
			STORE_ROOT=${2:?--store needs a directory}
			shift
			;;
		--config-dir)
			CONFIG_DIR=${2:?--config-dir needs a directory}
			shift
			;;
		--backup-root)
			BACKUP_ROOT=${2:?--backup-root needs a directory}
			shift
			;;
		--plugin-dir)
			PLUGIN_DIR=${2:?--plugin-dir needs a directory}
			shift
			;;
		--container-tag)
			CONTAINER_TAG=${2:?--container-tag needs a tag}
			shift
			;;
		--web-port)
			WEB_PORT_OVERRIDE=${2:?--web-port needs a number}
			shift
			;;
		--from)
			FROM_TS=${2:?--from needs a backup stamp}
			if ! valid_restore_stamp "$FROM_TS"; then
				printf 'live-swap: --from must be a backup stamp like 20261030T175230Z (got: %s)\n' "$FROM_TS" >&2
				exit 2
			fi
			shift
			;;
		-h | --help)
			usage
			exit 0
			;;
		*)
			printf 'live-swap: unknown argument: %s\n' "$1" >&2
			printf 'run live-swap.sh --help\n' >&2
			exit 2
			;;
		esac
		shift
	done
}


# ── path resolution and fail-closed validation ────────────────────────────
DATA_DIR=
LIVE_STORE_REAL=

resolve_paths() {
	# GNU coreutils are assumed throughout (stat -c, df --output, du -sb,
	# find -delete, sort -z); the runbook's host is Void Linux.
	if [ "$(uname -s)" != Linux ]; then
		die "this executor targets Linux only (GNU coreutils; the runbook's host is Void Linux)"
	fi
	if [ -n "$REHEARSE_ROOT" ]; then
		[ -d "$REHEARSE_ROOT" ] || die "--rehearse: not a directory: $REHEARSE_ROOT"
		REHEARSE_ROOT=$(abspath "$REHEARSE_ROOT")
		STORE_ROOT=$REHEARSE_ROOT/opencode-mem
		CONFIG_DIR=$REHEARSE_ROOT/config
		BACKUP_ROOT=$REHEARSE_ROOT/backups
		LIVE_STORE_REAL=$(abspath "$HOME/.opencode-mem")
		if is_under "$REHEARSE_ROOT" "$LIVE_STORE_REAL"; then
			die "--rehearse: $REHEARSE_ROOT is inside the live store $LIVE_STORE_REAL"
		fi
		if is_under "$LIVE_STORE_REAL" "$REHEARSE_ROOT"; then
			die "--rehearse: $REHEARSE_ROOT contains the live store $LIVE_STORE_REAL"
		fi
		if is_under "$(abspath "$HOME/.config/opencode")" "$REHEARSE_ROOT"; then
			die "--rehearse: $REHEARSE_ROOT contains the live config directory"
		fi
	fi

	STORE_ROOT=$(abspath "$STORE_ROOT")
	CONFIG_DIR=$(abspath "$CONFIG_DIR")
	BACKUP_ROOT=$(abspath "$BACKUP_ROOT")
	PLUGIN_DIR=$(abspath "$PLUGIN_DIR")
	DATA_DIR=$STORE_ROOT/data

	case $STORE_ROOT in
	/ | "$HOME") die "refusing to treat $STORE_ROOT as the store root" ;;
	esac
	case $DATA_DIR in
	/ | /data) die "refusing to treat $DATA_DIR as the store data directory" ;;
	esac
	if is_under "$BACKUP_ROOT" "$STORE_ROOT"; then
		die "the backup root must live OUTSIDE the store root ($BACKUP_ROOT is under $STORE_ROOT)"
	fi
	if [ "$(id -u)" -eq 0 ] && [ -z "$REHEARSE_ROOT" ]; then
		die "refusing to run as root: the runbook is a no-sudo, own-user procedure"
	fi
}

require_store() {
	[ -d "$STORE_ROOT" ] || die "store root not found: $STORE_ROOT (§1)"
	[ -d "$DATA_DIR" ] || die "store data directory not found: $DATA_DIR (§1)"
}

require_config() {
	[ -d "$CONFIG_DIR" ] || die "config directory not found: $CONFIG_DIR"
	[ -f "$CONFIG_DIR/opencode.jsonc" ] || die "missing $CONFIG_DIR/opencode.jsonc"
	[ -f "$CONFIG_DIR/opencode-mem.jsonc" ] || die "missing $CONFIG_DIR/opencode-mem.jsonc"
}

host_os() {
	local kernel=''
	kernel=$(uname -s)
	case $kernel in
	Linux) printf 'linux' ;;
	*) printf '' ;;
	esac
}

host_arch() {
	local machine=''
	machine=$(uname -m)
	case $machine in
	x86_64 | amd64) printf 'x64' ;;
	aarch64 | arm64) printf 'arm64' ;;
	*) printf '' ;;
	esac
}

engine_path() {
	local os='' arch=''
	os=$(host_os)
	arch=$(host_arch)
	if [ -z "$os" ] || [ -z "$arch" ]; then
		printf ''
		return 0
	fi
	printf '%s/engine/%s-%s/%s' "$PLUGIN_DIR" "$os" "$arch" "$ENGINE_NAME"
}

engine_is_stale() {
	local engine=''
	engine=$(engine_path)
	[ -n "$engine" ] && [ -x "$engine" ] && [ "$engine" -ot "$PLUGIN_DIR/dist/plugin.js" ]
}

require_plugin() {
	[ -d "$PLUGIN_DIR" ] || die "plugin directory not found: $PLUGIN_DIR"
	[ -f "$PLUGIN_DIR/dist/plugin.js" ] || die "plugin bundle not found: $PLUGIN_DIR/dist/plugin.js"
	local engine=''
	engine=$(engine_path)
	[ -n "$engine" ] || die "unsupported host: $(uname -s)/$(uname -m)"
	[ -x "$engine" ] || die "staged engine not found or not executable: $engine (run plugin/scripts/stage-engine.sh)"
	# The runbook's precondition for issue 81: the cutover must use an engine
	# rebuilt from landed main, not the rehearsal build. --swap reports this
	# before asking for confirmation, like --dry-run does.
	if engine_is_stale; then
		ck_warn "the staged engine is OLDER than $PLUGIN_DIR/dist/plugin.js — rebuild it first (plugin/scripts/stage-engine.sh, issue 81)"
	fi
}

# ── process scan (nothing here writes) ────────────────────────────────────
# /proc/<pid>/cmdline is NUL-separated. Only argv[0] and argv[1] are classified,
# so a `bash -c '... opencode ...'` command line cannot impersonate the host,
# and this script's own processes are skipped explicitly.
scan_procs() {
	local d='' pid='' base='' role='' token='' root='' i=0
	local -a args=()
	for d in /proc/[0-9]*; do
		pid=${d#/proc/}
		[ "$pid" = "$$" ] && continue
		[ -r "$d/cmdline" ] || continue
		args=()
		mapfile -d '' -t args <"$d/cmdline" 2>/dev/null || true
		[ "${#args[@]}" -gt 0 ] || continue
		role=
		base=${args[0]##*/}
		if [ "$base" = "$ENGINE_NAME" ]; then
			role=engine
		else
			for token in "${args[@]:0:2}"; do
				if [ "${token##*/}" = opencode ]; then
					role=host
					break
				fi
			done
		fi
		[ -n "$role" ] || continue
		for token in "${args[@]}"; do
			case $token in
			*live-swap.sh*)
				role=
				break
				;;
			esac
		done
		[ -n "$role" ] || continue
		root=-
		for ((i = 0; i < ${#args[@]} - 1; i++)); do
			if [ "${args[i]}" = --storage-root ]; then
				root=$(abspath "${args[i + 1]}")
				break
			fi
		done
		printf '%s\t%s\t%s\n' "$pid" "$role" "$root"
	done
	return 0
}

# One /proc sweep per invocation: the table is small and the modes call the
# two filters together. refresh_procs() is cheap to re-call before a decision.
PROC_TABLE=

refresh_procs() { PROC_TABLE=$(scan_procs); }

host_pids() {
	if [ -z "$PROC_TABLE" ]; then
		refresh_procs
	fi
	printf '%s\n' "$PROC_TABLE" | awk -F'\t' '$2 == "host" { print $1 }' | tr '\n' ' ' | sed -e 's/ *$//'
}

engine_pids_for_store() {
	local data_real=''
	data_real=$(abspath "$DATA_DIR")
	if [ -z "$PROC_TABLE" ]; then
		refresh_procs
	fi
	printf '%s\n' "$PROC_TABLE" | awk -F'\t' -v root="$data_real" '$2 == "engine" && $3 == root { print $1 }'
}

engine_ppid() {
	local ppid=''
	ppid=$(ps -o ppid= -p "$1" 2>/dev/null | tr -d ' ' || true)
	printf '%s' "$ppid"
}

engine_web_port() {
	local pid=$1 value='' i=0
	local -a args=()
	mapfile -d '' -t args <"/proc/$pid/cmdline" 2>/dev/null || true
	for ((i = 0; i < ${#args[@]} - 1; i++)); do
		if [ "${args[i]}" = --web-port ]; then
			printf '%s' "${args[i + 1]}"
			return 0
		fi
	done
	if [ -n "$WEB_PORT_OVERRIDE" ]; then
		printf '%s' "$WEB_PORT_OVERRIDE"
		return 0
	fi
	value=${OPENCODE_MEM_WEB_PORT:-}
	if [ -n "$value" ]; then
		printf '%s' "$value"
		return 0
	fi
	printf '3742'
}

engine_lock_pid() {
	local lock=$DATA_DIR/.engine.lock raw=''
	[ -f "$lock" ] || return 0
	raw=$(first_line '"pid"[[:space:]]*:[[:space:]]*[0-9]+' "$lock")
	printf '%s' "${raw##*:}" | tr -d ' '
}

pid_alive() {
	[ -n "${1:-}" ] && [ -d "/proc/$1" ]
}

require_stopped() {
	local engines='' hosts='' lock_pid=''
	refresh_procs
	engines=$(engine_pids_for_store | tr '\n' ' ' | sed -e 's/ *$//')
	hosts=$(host_pids)
	lock_pid=$(engine_lock_pid)
	if [ -n "$engines" ]; then
		die "an engine is still running for $DATA_DIR (pid ${engines}) — quit OpenCode and wait for it to exit (§2)"
	fi
	if [ -n "$lock_pid" ] && pid_alive "$lock_pid"; then
		die "$DATA_DIR/.engine.lock names a live pid ${lock_pid} — the store is still owned (§2)"
	fi
	if [ -n "$REHEARSE_ROOT" ]; then
		log "rehearsal: the live OpenCode process check is skipped (scratch store: $STORE_ROOT)"
		if [ -n "$hosts" ]; then
			log "rehearsal note: OpenCode is running (pid ${hosts}); nothing live is written"
		fi
		return 0
	fi
	if [ -n "$hosts" ]; then
		die "OpenCode is running (pid ${hosts}) — quit the application, then re-run (§2)"
	fi
	log "no OpenCode process and no engine for $DATA_DIR"
}

# ── store reads used by --dry-run and --verify ────────────────────────────
db_files() {
	local dir=$1
	find "$dir" -type f -name '*.db' | sort
}

db_user_version() {
	local db=$1 out=''
	out=$(sqlite3 -readonly "$db" 'PRAGMA user_version;' 2>&1 || true)
	printf '%s' "$out"
}

count_lines() {
	local n=0 out=''
	out=$1
	if [ -n "$out" ]; then
		n=$(printf '%s\n' "$out" | wc -l)
	fi
	printf '%s' "$n"
}

store_counts() {
	local dbs='' files='' shards=''
	dbs=$(count_lines "$(db_files "$DATA_DIR")")
	files=$(find "$STORE_ROOT" -type f | wc -l)
	shards=$(count_lines "$(find "$DATA_DIR/projects" -maxdepth 1 -type f -name '*.db' 2>/dev/null | sort)")
	printf 'dbs=%s files=%s project_shards=%s' "$dbs" "$files" "$shards"
}

wal_state() {
	local nonempty='' sidecars=''
	nonempty=$(count_lines "$(find "$DATA_DIR" -type f -name '*-wal' -size +0c 2>/dev/null | sort)")
	sidecars=$(count_lines "$(find "$DATA_DIR" -type f \( -name '*-shm' -o -name '*-tshm' \) 2>/dev/null | sort)")
	printf 'nonempty_wal=%s derived_sidecars=%s' "$nonempty" "$sidecars"
}

# ── §3 checkpoint ─────────────────────────────────────────────────────────
checkpoint_store() {
	heading '3. checkpoint the live DBs (runbook §3)'
	local -a dbs=()
	local db='' out='' busy='' wal='' ckpt='' failures=0 left=0
	mapfile -t dbs < <(db_files "$DATA_DIR")
	if [ "${#dbs[@]}" -eq 0 ]; then
		die "no *.db under $DATA_DIR — is this the store root? (§1)"
	fi
	log "checkpointing ${#dbs[@]} databases under $DATA_DIR"
	for db in "${dbs[@]}"; do
		if ! out=$(sqlite3 "$db" 'PRAGMA wal_checkpoint(TRUNCATE);' 2>&1); then
			printf '  %s -> %s\n' "${db#"$STORE_ROOT"/}" "$out"
			failures=$((failures + 1))
			continue
		fi
		busy=${out%%|*}
		wal=${out#*|}
		ckpt=${wal#*|}
		wal=${wal%%|*}
		printf '  %s -> %s\n' "${db#"$STORE_ROOT"/}" "$out"
		if [ "$busy" != 0 ] || [ "$ckpt" != "$wal" ]; then
			log "WARN checkpoint not clean for $db: $out"
			failures=$((failures + 1))
		fi
	done
	# Derived, rebuildable files only. Never a *.db, never a non-empty *.db-wal.
	log "removing the derived *-shm / *-tshm sidecars (rebuilt on the next open, §3)"
	find "$DATA_DIR" -type f \( -name '*-shm' -o -name '*-tshm' \) -delete ||
		die "cannot remove the derived sidecars in $DATA_DIR (§3)"
	left=$(count_lines "$(find "$DATA_DIR" -type f -name '*-wal' -size +0c | sort)")
	if [ "$left" -ne 0 ]; then
		die "$left non-empty *.db-wal file(s) remain after the checkpoint — a torn copy is likely; NOT deleting them (§3)"
	fi
	log "checkpoint done: ${#dbs[@]} databases, failures=$failures, non-empty WAL=0, sidecars removed"
	if [ "$failures" -ne 0 ]; then
		log "WARN $failures checkpoint(s) did not report 0|0|0 — the integrity step below will decide"
	fi
}

# ── §4 dated backup ───────────────────────────────────────────────────────
unique_dir() {
	local base=$1 candidate=$1 n=2
	while [ -e "$candidate" ]; do
		candidate="$base-$n"
		n=$((n + 1))
	done
	printf '%s' "$candidate"
}

make_backup() {
	local bytes='' free='' need='' backup='' ts=''
	checkpoint_store

	heading '4. dated backup of the whole root + both config files (runbook §4)'
	ts=$(utc_stamp)
	backup=$(unique_dir "$BACKUP_ROOT/$ts")
	# unique_dir may append -N on a same-second collision; the stamp that names
	# LATEST and the printed --from must be the directory that actually exists.
	ts=$(basename -- "$backup")
	mkdir -p "$BACKUP_ROOT" || die "cannot create the backup root: $BACKUP_ROOT"
	bytes=$(du -sb "$STORE_ROOT" | cut -f1)
	free=$(df -B1 --output=avail "$BACKUP_ROOT" | sed -n '2p' | tr -d ' ')
	need=$((bytes * 2))
	if [ "$free" -lt "$need" ]; then
		die "not enough free space for the backup: need ~2x $bytes B, have $free B (§1)"
	fi
	log "backup root $BACKUP_ROOT: free=$free need=$need store=$bytes B"
	log "cp -a $STORE_ROOT $backup"
	cp -a "$STORE_ROOT" "$backup" || die "cp -a of the store failed: $backup"
	mkdir -p "$backup/config"
	cp -a "$CONFIG_DIR/opencode.jsonc" "$backup/config/opencode.jsonc" || die "cannot copy opencode.jsonc"
	cp -a "$CONFIG_DIR/opencode-mem.jsonc" "$backup/config/opencode-mem.jsonc" || die "cannot copy opencode-mem.jsonc"
	BACKUP_DIR=$backup
	log "backup = $backup (LATEST is written only after §5 passes)"
	du -sh -- "$backup"
	printf 'files=%s dbs=%s\n' \
		"$(find "$backup" -type f | wc -l)" \
		"$(count_lines "$(db_files "$backup/data")")"

	heading '5. manifest, verify, per-DB integrity (runbook §5)'
	local manifest=
	manifest=$(mktemp) || die 'cannot create a temp file for the manifest'
	(
		cd "$backup" || exit 1
		find . -type f ! -name MANIFEST.sha256 ! -name BACKUP-INFO -print0 |
			sort -z | xargs -0 -r sha256sum
	) >"$manifest" || die 'cannot build MANIFEST.sha256'
	mv "$manifest" "$backup/MANIFEST.sha256" || die 'cannot write MANIFEST.sha256'
	local lines=''
	lines=$(wc -l <"$backup/MANIFEST.sha256" | tr -d ' ')
	log "MANIFEST.sha256: $lines entries"
	if ! (cd "$backup" && sha256sum -c MANIFEST.sha256 --quiet); then
		die "the manifest does NOT verify — this is not a restore point; fix the copy (§5)"
	fi
	log "sha256sum -c: 0 failed ($lines entries)"

	local -a dbs=()
	mapfile -t dbs < <(db_files "$backup/data")
	local db='' out='' ok=0 bad=0
	for db in "${dbs[@]}"; do
		out=$(sqlite3 -readonly "file:$db?immutable=1" 'PRAGMA integrity_check;' 2>&1 || true)
		printf '  %s %s\n' "${db#"$backup"/}" "$out"
		if [ "$out" = ok ]; then
			ok=$((ok + 1))
		else
			bad=$((bad + 1))
		fi
	done
	log "integrity_check: ${#dbs[@]} databases, ok=$ok, failures=$bad"
	if [ "${#dbs[@]}" -eq 0 ] || [ "$bad" -ne 0 ]; then
		die "per-DB integrity_check failed ($bad of ${#dbs[@]}) — the backup is not a valid restore point (§5)"
	fi
	ck_ok "every copied database passes integrity_check ($ok/$ok)"

	{
		printf 'ts=%s\n' "$ts"
		printf 'store_root=%s\n' "$STORE_ROOT"
		printf 'config_dir=%s\n' "$CONFIG_DIR"
		printf 'engine_sha256=%s\n' "$(sha256_of "$(engine_path)")"
		printf 'commit=%s\n' "$(git_commit)"
		printf 'manifest_entries=%s\n' "$lines"
		printf 'dbs=%s\n' "${#dbs[@]}"
	} >"$backup/BACKUP-INFO"
	log "wrote $backup/BACKUP-INFO"

	# A restore point is a directory that verified, so LATEST moves only now.
	printf '%s\n' "$ts" >"$BACKUP_ROOT/LATEST"
	BACKUP_DIR=$backup
	log "LATEST -> $ts"

	printf '\nbackup path: %s\n' "$backup"
	printf 'rollback   : %s --rollback   (or --from %s)\n' "$SCRIPT_PATH" "$ts"
}

BACKUP_DIR=

sha256_of() {
	local file=$1
	[ -f "$file" ] || return 0
	sha256sum -- "$file" | cut -d' ' -f1
}

git_commit() {
	local commit=''
	commit=$(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || true)
	printf '%s' "${commit:-unknown}"
}

# ── §8 plugin-entry swap ──────────────────────────────────────────────────
FROM_TOKEN='"opencode-mem@latest"'

count_token() {
	local file=$1 token=$2 n=''
	n=$(grep -oF -- "$token" "$file" 2>/dev/null | wc -l | tr -d ' ' || true)
	printf '%s' "${n:-0}"
}

# count_active_entry <file> <quoted-token> — the number of plugin-array lines
# whose whole trimmed text is the token (optionally with the trailing comma).
# A commented-out line (`// "..."`) is not an active entry, and the live
# opencode.jsonc does carry one such comment.
count_active_entry() {
	local file=$1 token=$2 line='' trimmed='' n=0
	while IFS= read -r line || [ -n "$line" ]; do
		trimmed=${line#"${line%%[![:space:]]*}"}
		trimmed=${trimmed%"${trimmed##*[![:space:]]}"}
		if [ "$trimmed" = "$token" ] || [ "$trimmed" = "$token," ]; then
			n=$((n + 1))
		fi
	done <"$file"
	printf '%s' "$n"
}

replace_token() {
	# Exact literal replacement of the first occurrence on a line; every other
	# byte (indentation, comments, commas) is preserved.
	local file=$1 from=$2 to=$3
	awk -v from="$from" -v to="$to" '
		{
			i = index($0, from)
			if (i > 0) {
				$0 = substr($0, 1, i - 1) to substr($0, i + length(from))
			}
			print
		}
	' "$file"
}

swap_plugin_entry() {
	local config=$CONFIG_DIR/opencode.jsonc
	local to="\"$PLUGIN_DIR\""
	local old_n='' new_n='' pre='' tmp='' expect_ts='' changed=''

	heading '8. swap the plugin entry (runbook §8)'
	old_n=$(count_active_entry "$config" "$FROM_TOKEN")
	new_n=$(count_active_entry "$config" "$to")
	log "plugin entries: active $FROM_TOKEN=$old_n, active $to=$new_n (commented lines do not count)"
	if [ "$old_n" -eq 0 ] && [ "$new_n" -eq 1 ]; then
		log "the plugin entry is already the local path ($to) — nothing to change (idempotent)"
		ck_info "config already references $PLUGIN_DIR"
		local previous=''
		previous=$(find "$CONFIG_DIR" -maxdepth 1 -type f -name 'opencode.jsonc.pre-swap-*' | sort | tail -n 1 || true)
		if [ -n "$previous" ]; then
			printf 'restore command: cp -a %s %s\n' "$previous" "$config"
		else
			printf 'restore command: cp -a %s %s   (from the latest backup)\n' \
				"${BACKUP_DIR:-$BACKUP_ROOT/<TS>}/config/opencode.jsonc" "$config"
		fi
		return 0
	fi
	if [ "$old_n" -gt 1 ]; then
		die "$old_n occurrences of $FROM_TOKEN in $config — ambiguous, refusing to guess (§8)"
	fi
	if [ "$old_n" -eq 0 ]; then
		die "no active $FROM_TOKEN entry in $config — not the runbook's file (§8); nothing changed"
	fi
	if [ "$new_n" -ge 1 ]; then
		die "an active $to entry already exists in $config ($new_n) beside $FROM_TOKEN — refusing a double edit (§8)"
	fi

	printf '\nplanned edit of %s:\n' "$config"
	printf '  - %s\n' "$FROM_TOKEN"
	printf '  + %s\n' "$to"
	printf '  (one line, one string; commas, comments and layout untouched)\n'
	printf 'the store migration happens at the NEXT start (runbook §6); this script does not start OpenCode.\n'
	confirm "replace $FROM_TOKEN with $to in $config (copy kept as .pre-swap-<TS>)"
	if [ "$CONFIRMED" -ne 1 ]; then
		log 'the owner declined the config edit; the backup above is intact'
		exit 130
	fi

	expect_ts=$(utc_stamp)
	pre=$config.pre-swap-$expect_ts
	cp -a "$config" "$pre" || die "cannot write the pre-swap copy: $pre"
	log "pre-swap copy: $pre"
	if [ "$(sha256_of "$pre")" != "$(sha256_of "$config")" ]; then
		die "the pre-swap copy does not match the config — nothing edited"
	fi

	tmp=$(mktemp "$CONFIG_DIR/.opencode.jsonc.new.XXXXXX") || die "cannot create a temp file in $CONFIG_DIR"
	trap 'find "$CONFIG_DIR" -maxdepth 1 -name ".opencode.jsonc.new.*" -delete 2>/dev/null || true' EXIT
	replace_token "$config" "$FROM_TOKEN" "$to" >"$tmp" || die "the replacement failed; nothing edited"
	if [ "$(count_active_entry "$tmp" "$FROM_TOKEN")" -ne 0 ]; then
		die "the replacement left an active $FROM_TOKEN entry behind; nothing edited"
	fi
	if [ "$(count_active_entry "$tmp" "$to")" -ne 1 ]; then
		die "the replacement did not produce exactly one active $to entry; nothing edited"
	fi
	changed=$( { diff --unchanged-line-format='' --old-line-format='-%L' --new-line-format='+%L' "$config" "$tmp" || true; } | wc -l | tr -d ' ')
	if [ "$changed" != 2 ]; then
		die "the edit would change $((changed / 2)) line(s), not exactly one — refusing (§8); nothing edited"
	fi
	chmod "$(stat -c %a "$config")" "$tmp"
	mv -f "$tmp" "$config" || die "cannot replace $config"
	trap - EXIT
	log "swapped the plugin entry in $config"
	printf '\n--- diff (pre-swap -> current) ---\n'
	diff -u "$pre" "$config" || true
	printf '\nrestore command (any time):\n  cp -a %s %s\n' "$pre" "$config"
	printf 'backup (whole store + both configs):\n  %s\n' "${BACKUP_DIR:-$BACKUP_ROOT}"
}

# ── §7 provisioning ───────────────────────────────────────────────────────
provision_cache() {
	heading '7. provision the ONNX cache (runbook §7)'
	if [ -n "$REHEARSE_ROOT" ]; then
		log "rehearsal: skipping $PROVISIONER (it writes ${TMPDIR:-/tmp}, outside the scratch root)"
		return 0
	fi
	[ -f "$PROVISIONER" ] || die "provisioner not found: $PROVISIONER"
	log "running $PROVISIONER (measured ~38 s cold)"
	local out=''
	if ! out=$(sh "$PROVISIONER" 2>&1); then
		printf '%s\n' "$out" | tail -n 40 >&2
		die "the ONNX provisioner failed — search/add would answer -32603 (§7)"
	fi
	printf '%s\n' "$out" | tail -n 6
	case $out in
	*tokenizer_version=2.0*) ck_ok 'tokenizer canonicalized to v2.0' ;;
	*) die "the provisioner did not report tokenizer_version=2.0 (§7)" ;;
	esac
	case $out in
	*model_onnx_sha256="$EXPECTED_ONNX_SHA"*) ck_ok 'model_onnx sha256 matches the runbook' ;;
	*) ck_warn 'the provisioned model_onnx sha256 differs from the runbook figure' ;;
	esac
}

# ── modes ─────────────────────────────────────────────────────────────────
cache_dir() {
	local model=$1
	printf '%s/.opencode-mem-onnx/%s' "${TMPDIR:-/tmp}" "$model"
}

tokenizer_version_of() {
	local file=$1 raw=''
	[ -f "$file" ] || return 0
	raw=$(first_line '"version"[[:space:]]*:[[:space:]]*"[0-9][0-9.]*"' "$file")
	[ -n "$raw" ] || return 0
	printf '%s' "$raw" | sed -E 's/.*"([0-9][0-9.]*)"$/\1/'
}

dry_run() {
	heading 'live-swap --dry-run (reads only; nothing is written)'
	log "store=$STORE_ROOT config=$CONFIG_DIR backups=$BACKUP_ROOT plugin=$PLUGIN_DIR"
	if [ -n "$REHEARSE_ROOT" ]; then
		log "rehearsal root=$REHEARSE_ROOT (scratch paths, not the live ones)"
	fi

	heading '1. preflight (runbook §1)'
	if [ -d "$STORE_ROOT" ] && [ -d "$DATA_DIR" ]; then
		ck_ok "store root exists: $STORE_ROOT"
		ck_info "$(store_counts)"
		ck_info "wal state: $(wal_state)"
	else
		ck_fail "store root/data not found: $STORE_ROOT (expected \$HOME/.opencode-mem/data) — pass --store to override"
	fi
	if [ -d "$DATA_DIR/.cache/Xenova/nomic-embed-text-v1" ]; then
		ck_ok 'the upstream .cache model directory exists (the provisioner source, §7)'
	else
		ck_warn 'no data/.cache/Xenova/nomic-embed-text-v1 — the provisioner will have no source (§7)'
	fi
	if [ -d "$STORE_ROOT" ]; then
		local bytes='' free='' need=''
		bytes=$(du -sb "$STORE_ROOT" | cut -f1)
		free=$(df -B1 --output=avail "$STORE_ROOT" | sed -n '2p' | tr -d ' ')
		need=$((bytes * 2))
		if [ "$free" -ge "$need" ]; then
			ck_ok "disk: store=$bytes B, free=$free B, need ~$need B (2x, §1)"
		else
			ck_fail "disk: need ~$need B free for the backup, have $free B (§1)"
		fi
	fi

	heading '1b. config values (runbook §1, §8)'
	if [ -f "$CONFIG_DIR/opencode-mem.jsonc" ] && [ -f "$CONFIG_DIR/opencode.jsonc" ]; then
		local storage='' model='' port='' entry_old='' entry_new=''
		storage=$(config_value "$CONFIG_DIR/opencode-mem.jsonc" storagePath)
		model=$(config_value "$CONFIG_DIR/opencode-mem.jsonc" embeddingModel)
		port=$(config_number "$CONFIG_DIR/opencode-mem.jsonc" webServerPort)
		ck_info "storagePath    = ${storage:-<unset>} (the plugin resolves \$HOME/.opencode-mem/data, §1)"
		ck_info "embeddingModel = ${model:-<unset>}"
		ck_info "webServerPort  = ${port:-<unset>} (NOT forwarded by the plugin launch; the engine binds 3742, §1)"
		entry_old=$(count_active_entry "$CONFIG_DIR/opencode.jsonc" "$FROM_TOKEN")
		entry_new=$(count_active_entry "$CONFIG_DIR/opencode.jsonc" "\"$PLUGIN_DIR\"")
		if [ "$entry_old" -eq 1 ] && [ "$entry_new" -eq 0 ]; then
			ck_pending 'opencode.jsonc still loads upstream opencode-mem@latest — --swap replaces it (§8)'
			if [ "$(count_token "$CONFIG_DIR/opencode.jsonc" "\"$PLUGIN_DIR\"")" -ge 1 ]; then
				ck_info 'the local path also appears as a comment — commented lines are not entries'
			fi
		elif [ "$entry_old" -eq 0 ] && [ "$entry_new" -eq 1 ]; then
			ck_ok 'opencode.jsonc already references the local plugin path'
		else
			ck_fail "opencode.jsonc has $entry_old active $FROM_TOKEN and $entry_new active $PLUGIN_DIR entries — not the runbook's shape (§8)"
		fi
	else
		ck_fail "missing $CONFIG_DIR/opencode.jsonc or opencode-mem.jsonc"
	fi

	heading '1c. plugin dist + staged engine (§8, issue 81)'
	if [ -d "$PLUGIN_DIR" ]; then
		if [ -f "$PLUGIN_DIR/dist/plugin.js" ]; then
			ck_ok "plugin bundle: $PLUGIN_DIR/dist/plugin.js ($(wc -c <"$PLUGIN_DIR/dist/plugin.js" | tr -d ' ') B)"
		else
			ck_fail "plugin bundle missing: $PLUGIN_DIR/dist/plugin.js"
		fi
		local engine='' webdist=''
		engine=$(engine_path)
		if [ -n "$engine" ] && [ -x "$engine" ]; then
			ck_ok "staged engine: $engine"
			ck_info "engine sha256=$(sha256_of "$engine") ($(stat -c %y "$engine"))"
			webdist=$(dirname "$engine")/web-ui/dist/index.html
			if [ -f "$webdist" ]; then
				ck_ok 'web UI bundle staged beside the engine'
			else
				ck_fail "no web UI bundle beside the engine ($webdist) — the web surface degrades (§9/issue 36)"
			fi
			if engine_is_stale; then
				ck_warn 'the staged engine is OLDER than dist/plugin.js — rebuild it from landed main (plugin/scripts/stage-engine.sh, issue 81)'
			fi
		else
			ck_fail "no staged engine at $engine — run plugin/scripts/stage-engine.sh"
		fi
		if [ -f "$PLUGIN_DIR/scripts/stage-engine.sh" ]; then
			ck_info "rebuild: sh $PLUGIN_DIR/scripts/stage-engine.sh"
		fi
	else
		ck_fail "plugin directory not found: $PLUGIN_DIR"
	fi

	heading '1d. is anything running? (runbook §2)'
	local engines='' hosts='' lock_pid=''
	refresh_procs
	engines=$(engine_pids_for_store | tr '\n' ' ' | sed -e 's/ *$//')
	hosts=$(host_pids)
	lock_pid=$(engine_lock_pid)
	if [ -n "$engines" ]; then
		ck_pending "engine running for this store: pid ${engines} — --backup/--swap/--rollback will refuse"
	else
		ck_ok 'no engine process for this store'
	fi
	if [ -n "$hosts" ] && [ -n "$REHEARSE_ROOT" ]; then
		ck_info "OpenCode running: pid ${hosts} — irrelevant to these scratch paths (rehearsal writes nothing live)"
	elif [ -n "$hosts" ]; then
		ck_pending "OpenCode running: pid ${hosts} — quit it before --backup/--swap/--rollback (§2)"
	else
		ck_ok 'no OpenCode process'
	fi
	if [ -n "$lock_pid" ] && pid_alive "$lock_pid"; then
		ck_pending "store lock names a live pid ${lock_pid} (§7 bridge-schema)"
	fi
	if [ -n "$hosts" ] || [ -n "$engines" ]; then
		ck_info '--dry-run and --verify are safe now; the other modes are not'
	fi

	heading '1e. backup root (§4)'
	if [ -d "$BACKUP_ROOT" ]; then
		if [ -w "$BACKUP_ROOT" ]; then
			ck_ok "backup root writable: $BACKUP_ROOT"
		else
			ck_fail "backup root is not writable: $BACKUP_ROOT"
		fi
		if [ -f "$BACKUP_ROOT/LATEST" ]; then
			local latest=''
			latest=$(cat "$BACKUP_ROOT/LATEST")
			ck_info "LATEST = $latest"
			if [ -f "$BACKUP_ROOT/$latest/config/opencode.jsonc" ] &&
				[ "$(count_active_entry "$BACKUP_ROOT/$latest/config/opencode.jsonc" "\"$PLUGIN_DIR\"")" -ge 1 ]; then
				ck_warn "LATEST was taken AFTER the swap (its config already points at the port) — --rollback needs --from an earlier backup (§12)"
			fi
		else
			ck_warn "no LATEST pointer yet — --backup writes one. A bare --rollback only falls back to a dated backup holding BOTH MANIFEST.sha256 and BACKUP-INFO; a hand-made backup without BACKUP-INFO (the live machine's 20261030T175230Z is one) is not used — pass --from <TS> (§12)"
		fi
		local newest=''
		newest=$(find "$BACKUP_ROOT" -maxdepth 1 -type d -name '20*' | sort | tail -n 1 || true)
		if [ -n "$newest" ]; then
			ck_info "newest backup: $(basename -- "$newest") — $(du -sh "$newest" | cut -f1), $(find "$newest" -type f | wc -l) files"
		fi
	else
		local parent=''
		parent=$(dirname -- "$BACKUP_ROOT")
		if [ -w "$parent" ]; then
			ck_ok "backup root will be created under a writable parent: $parent"
		else
			ck_fail "cannot create the backup root: $parent is not writable"
		fi
	fi
	if is_under "$BACKUP_ROOT" "$STORE_ROOT"; then
		ck_fail "the backup root is inside the store root — refusing (§4)"
	else
		ck_ok "the backup root is outside the store root (required, §4)"
	fi

	heading '1f. ONNX cache state (§7)'
	local model_id='' cache='' tok='' onnx=''
	model_id=$(config_value "$CONFIG_DIR/opencode-mem.jsonc" embeddingModel)
	if [ -z "$model_id" ]; then
		model_id=Xenova/nomic-embed-text-v1
	fi
	cache=$(cache_dir "$model_id")
	onnx=$cache/onnx/model.onnx
	if [ -f "$onnx" ]; then
		tok=$(tokenizer_version_of "$cache/tokenizer.json")
		local have=''
		have=$(wc -c <"$onnx" | tr -d ' ')
		ck_info "cache: $cache (tokenizer_version=${tok:-unknown}, model_onnx_bytes=$have)"
		if [ "$tok" = 2.0 ]; then
			ck_ok 'tokenizer is the canonical v2.0 form the rc.2 reader accepts'
		else
			ck_pending "tokenizer is version ${tok:-unknown} — search/add would answer -32603 until provisioned (§7)"
		fi
		if [ "$have" != "$EXPECTED_ONNX_BYTES" ]; then
			ck_warn "model_onnx is $have B, the runbook measured $EXPECTED_ONNX_BYTES B"
		fi
	else
		ck_pending "no cache at $cache — --swap provisions it (§7, ~38 s); re-run it after every reboot"
	fi

	heading 'planned actions (nothing here runs during --dry-run)'
	plan 'quit OpenCode; confirm with pgrep -af opencode (§2)'
	plan "sqlite3 <each of the $(count_lines "$(db_files "$DATA_DIR")") *.db> 'PRAGMA wal_checkpoint(TRUNCATE);' (§3)"
	plan 'find data -name "*-shm" -o -name "*-tshm" -delete   (never a .db, never a non-empty -wal) (§3)'
	plan "cp -a $STORE_ROOT $BACKUP_ROOT/<UTC>   +   both config files into <UTC>/config/ (§4)"
	plan 'sha256 manifest over every file, then sha256sum -c (§5)'
	plan 'PRAGMA integrity_check per DB, read with immutable=1 (§5)'
	plan "printf '<UTC>' > $BACKUP_ROOT/LATEST   (only after §5 verifies, §4)"
	plan "sh $PROVISIONER (§7)"
	plan "cp -a $CONFIG_DIR/opencode.jsonc <same>.pre-swap-<TS>   then replace $FROM_TOKEN with \"$PLUGIN_DIR\" (§8)"
	plan 'start OpenCode; the engine migrates in place (metadata 1->2, prompts 1->2, shards 2->3 lazily, §6)'
	plan 'quit and start OpenCode once more (the fresh-engine reopen, §9)'
	plan "$SCRIPT_PATH --verify   (§9b/§10)"
	plan "keep the backup untouched for one week of dogfood (§11); roll back with $SCRIPT_PATH --rollback (§12)"

	heading 'summary'
	printf '  failures=%s warnings=%s pending=false-start-conditions=%s\n' "$FAILS" "$WARNINGS" "$PENDING"
	if [ "$FAILS" -ne 0 ]; then
		printf '  a FAIL line must be fixed before any mode touches the live host\n'
	fi
}

# Every writing mode opens its log only AFTER require_stopped: a refused run
# must not have created the backup root or written swap.log outside the store.
begin_writing_log() {
	mkdir -p "$BACKUP_ROOT" || die "cannot create the backup root: $BACKUP_ROOT"
	LOG_FILE=$BACKUP_ROOT/swap.log
	log "live-swap.sh mode=$MODE store=$STORE_ROOT backups=$BACKUP_ROOT commit=$(git_commit)"
}

run_backup_mode() {
	require_store
	require_config
	require_stopped
	begin_writing_log
	local bytes=''
	bytes=$(du -sb "$STORE_ROOT" | cut -f1)
	log "about to checkpoint and copy $STORE_ROOT ($bytes B) to $BACKUP_ROOT"
	confirm "checkpoint the live store, delete the derived *-shm/*-tshm sidecars, and copy the whole root to $BACKUP_ROOT/<UTC>"
	if [ "$CONFIRMED" -ne 1 ]; then
		log 'the owner declined; nothing was written'
		exit 130
	fi
	make_backup
	heading 'done'
	log 'backup complete; the store was not migrated (that is the next start, §6)'
	printf 'do not start OpenCode: run %s --swap next (or re-run --backup for a newer backup).\n' "$SCRIPT_PATH"
}

run_swap_mode() {
	require_store
	require_config
	require_plugin
	require_stopped
	begin_writing_log
	local stale=''
	if engine_is_stale; then
		stale=' WARNING: the staged engine is OLDER than plugin/dist/plugin.js — rebuild it first (plugin/scripts/stage-engine.sh, issue 81).'
	fi
	log "swap: plugin=$PLUGIN_DIR engine_sha256=$(sha256_of "$(engine_path)") commit=$(git_commit)"
	printf '\nthis runs: backup (§3-§5) -> provision (§7) -> plugin-entry swap (§8).\n'
	printf 'it does NOT start OpenCode; you do, then you run --verify.\n'
	confirm "checkpoint+back up the live store, provision the ONNX cache, and replace $FROM_TOKEN with \"$PLUGIN_DIR\" in $CONFIG_DIR/opencode.jsonc.${stale}"
	if [ "$CONFIRMED" -ne 1 ]; then
		log 'the owner declined; nothing was written'
		exit 130
	fi
	make_backup
	provision_cache
	swap_plugin_entry

	heading 'next'
	printf 'start OpenCode now, then run:\n  %s --verify\n' "$SCRIPT_PATH"
	printf 'expected on the first start (§6, §9): the memory tool answers, the web UI loads, no engine\n'
	printf 'stderr beyond consolidation.startup_tick / engine.shutdown_complete. Then quit and start\n'
	printf 'once more (the fresh-engine reopen) before the two-workspace check in --verify.\n'
	printf 'store migration happens at that start; the backup above is the rollback (§12).\n'
}

HTTP_CODE=
PROBE_BODY=

# probe <port> <path> [curl args...] — sets HTTP_CODE and PROBE_BODY.
# Not a command substitution on purpose: a subshell would drop the globals.
probe() {
	local port=$1 path=$2
	shift 2
	local tmp=''
	tmp=$(mktemp) || die 'cannot create a temp file'
	HTTP_CODE=$(curl -sS -o "$tmp" -w '%{http_code}' --max-time 20 "$@" "http://127.0.0.1:$port$path" || true)
	PROBE_BODY=$(cat "$tmp" 2>/dev/null || true)
	find "$tmp" -delete
}

run_verify_mode() {
	require_store
	if ! command -v jq >/dev/null 2>&1; then
		die '--verify needs jq to read the JSON envelopes (install jq)'
	fi
	heading 'verify (runbook §9b/§10) — after OpenCode started'

	local -a engines=()
	mapfile -t engines < <(engine_pids_for_store)
	local host_list=''
	host_list=$(host_pids)
	ck_info "OpenCode host pid(s): ${host_list:-none}"
	if [ "${#engines[@]}" -eq 0 ]; then
		ck_fail "no engine is running for $DATA_DIR — start OpenCode (and check the plugin entry points at $PLUGIN_DIR)"
		verify_summary
		return 0
	fi
	if [ "${#engines[@]}" -eq 1 ]; then
		ck_ok "exactly one engine for this store: pid ${engines[0]}"
	else
		ck_fail "${#engines[@]} engines for this store (pid ${engines[*]}) — the single-engine lock is defeated (§9b fact 2)"
	fi

	local pid=${engines[0]} ppid=''
	ppid=$(engine_ppid "$pid")
	if [ -z "$ppid" ] || [ "$ppid" = 0 ]; then
		ck_fail "cannot read the engine's parent pid (ps -o ppid= -p $pid)"
	else
		local found=0 candidate=0
		for candidate in $host_list; do
			if [ "$candidate" = "$ppid" ]; then
				found=1
			fi
		done
		if [ "$found" -eq 1 ]; then
			ck_ok "the engine's parent is the OpenCode host (ppid=$ppid) — the one-process premise, §9b fact 3"
		elif [ -n "$REHEARSE_ROOT" ]; then
			ck_info "rehearsal: the engine's parent (ppid=$ppid) is this shell, not an OpenCode host — on the live host this is the FAIL that reopens the socket-service question (§9b fact 3)"
		else
			ck_fail "the engine's parent (ppid=$ppid) is NOT an OpenCode host process (${host_list:-none}) — §9b fact 3 fails: stop, record, reopen the socket-service alternative"
		fi
	fi

	local lock_pid=''
	lock_pid=$(engine_lock_pid)
	if [ "$lock_pid" = "$pid" ]; then
		ck_ok "the store lock names the running engine (pid $lock_pid)"
	elif [ -n "$lock_pid" ]; then
		ck_warn "the store lock names pid $lock_pid, the running engine is $pid"
	else
		ck_warn 'no readable .engine.lock body'
	fi

	heading 'web surface (runbook §1, §10 rows 11-12)'
	local port=''
	port=$(engine_web_port "$pid")
	ck_info "engine web port: $port (config webServerPort 4747 is not forwarded)"
	probe "$port" /api/health
	if [ "$HTTP_CODE" = 200 ] && printf '%s' "$PROBE_BODY" | jq -e '.success == true and .status == "ok"' >/dev/null 2>&1; then
		ck_ok "/api/health = $PROBE_BODY"
	else
		ck_fail "/api/health on port $port answered HTTP ${HTTP_CODE} body '$PROBE_BODY' (expected 200 {\"success\":true,\"status\":\"ok\"})"
	fi

	local token_file=$DATA_DIR/.auth-token token=''
	probe "$port" /api/stats
	if [ "$HTTP_CODE" = 200 ]; then
		ck_ok '/api/stats answered 200 (an already-minted token was accepted)'
	elif [ "$HTTP_CODE" = 401 ]; then
		ck_ok '/api/stats answered 401 — the first gated call, which mints the token (§7)'
	else
		ck_fail "/api/stats answered HTTP ${HTTP_CODE} (expected 200, or 401 on the minting call)"
	fi
	if [ -f "$token_file" ]; then
		token=$(tr -d '\n' <"$token_file")
		ck_ok "token minted at $token_file (${#token} chars) — NOT ~/.opencode-mem/.auth-token"
		if [ -z "$token" ]; then
			ck_fail "the token file $token_file is empty"
		fi
	else
		ck_fail "no token at $token_file — the minting call did not create it (§7)"
	fi

	if [ -n "$token" ]; then
		heading 'search + add canary (runbook §7, battery row 12)'
		probe "$port" /api/search --get --data-urlencode 'q=W8 brute-force search cost benchmark shard layout' -H "x-opencode-mem-token: $token"
		if [ "$HTTP_CODE" = 200 ] && printf '%s' "$PROBE_BODY" | jq -e '.success == true and (.data.items | type) == "array"' >/dev/null 2>&1; then
			local hits='' top=''
			hits=$(printf '%s' "$PROBE_BODY" | jq -r '.data.items | length')
			top=$(printf '%s' "$PROBE_BODY" | jq -r '.data.items[0].similarity // "n/a"')
			if [ "$hits" -gt 0 ]; then
				ck_ok "search works: $hits hit(s), top similarity $top (runbook: rank 1 ≈0.54)"
				ck_info "top id: $(printf '%s' "$PROBE_BODY" | jq -r '.data.items[0].id // "n/a"')"
			else
				ck_warn 'search answered 200 with no hits — search is wired; the probe query simply is not in this store'
			fi
		else
			ck_fail "search answered HTTP ${HTTP_CODE}: $(printf '%s' "$PROBE_BODY" | head -c 300)"
		fi

		local canary='' canary_id=''
		canary="live-swap canary $(now_utc)"
		local payload=''
		payload=$(jq -cn --arg content "$canary" --arg tag "$CONTAINER_TAG" '{content: $content, containerTag: $tag}')
		probe "$port" /api/memories -X POST -H "x-opencode-mem-token: $token" -H 'content-type: application/json' -d "$payload"
		if [ "$HTTP_CODE" = 200 ] && printf '%s' "$PROBE_BODY" | jq -e '.success == true' >/dev/null 2>&1; then
			canary_id=$(printf '%s' "$PROBE_BODY" | jq -r '.data.id // empty')
			ck_ok "add works: canary stored as ${canary_id:-<no id in the envelope>} (tag $CONTAINER_TAG)"
			ck_info "the canary is one real memory row; delete it in the web UI if you do not want it"
		else
			ck_fail "add answered HTTP ${HTTP_CODE}: $(printf '%s' "$PROBE_BODY" | head -c 300) — a -32603 here is the cache (§7)"
		fi
		if [ -n "$canary_id" ]; then
			probe "$port" /api/search --get --data-urlencode "q=$canary" -H "x-opencode-mem-token: $token"
			if printf '%s' "$PROBE_BODY" | jq -e --arg id "$canary_id" '[.data.items[].id] | index($id) != null' >/dev/null 2>&1; then
				local rank=''
				rank=$(printf '%s' "$PROBE_BODY" | jq -r --arg id "$canary_id" '[.data.items[].id] | index($id) + 1')
				ck_ok "the canary is findable (rank $rank) — add then search round-trips"
			else
				ck_fail 'the canary was stored but not found by search'
			fi
		fi
	fi

	heading 'store versions (runbook §6) — after the canary touched the store'
	local meta=$DATA_DIR/metadata.db prompts=$DATA_DIR/user-prompts.db version=''
	if [ -f "$meta" ]; then
		version=$(db_user_version "$meta")
		if [ "$version" = 2 ]; then
			ck_ok 'metadata.db is version 2 (migrated)'
		else
			ck_fail "metadata.db is version ${version:-unknown}, expected 2 — the port has not touched the store (open the memory tool once, §6)"
		fi
	else
		ck_fail "metadata.db not found: $meta"
	fi
	if [ -f "$prompts" ]; then
		version=$(db_user_version "$prompts")
		if [ "$version" = 2 ]; then
			ck_ok 'user-prompts.db is version 2'
		else
			ck_warn "user-prompts.db is version ${version:-unknown}, expected 2 — it stamps on the first web read with ?includePrompts=true (§6)"
		fi
	fi
	local -a shards=()
	local shard='' v3='' v2='' other=0
	mapfile -t shards < <(find "$DATA_DIR/projects" -maxdepth 1 -type f -name '*.db' 2>/dev/null | sort || true)
	for shard in "${shards[@]}"; do
		version=$(db_user_version "$shard")
		case $version in
		3) v3=$((v3 + 1)) ;;
		2) v2=$((v2 + 1)) ;;
		*) other=$((other + 1)) ;;
		esac
	done
	ck_info "project shards: total=${#shards[@]} v3=$v3 v2=$v2 other=$other (2->3 is lazy, on the first write to that project, §6)"

	heading 'status file (bridge-schema §7)'
	local status_file=$DATA_DIR/status status_pid=''
	if [ -f "$status_file" ]; then
		local line=''
		line=$(head -n 1 "$status_file" || true)
		ck_info "status: $line"
		status_pid=$(printf '%s' "$line" | sed -n 's/.*(pid=\([0-9][0-9]*\), ts=.*/\1/p')
		if [ -z "$status_pid" ]; then
			ck_warn "the status file does not carry the (pid=<n>, ts=<...>) grammar"
		elif pid_alive "$status_pid"; then
			ck_fail "the status file names a LIVE pid $status_pid — a genuine loser wrote it; some agent is degraded (§7)"
		else
			ck_ok "the status file names pid $status_pid, which is dead — a stale report, not a live degradation"
		fi
	else
		ck_ok 'no status file — no instance reported a degradation'
	fi

	heading 'store counts'
	ck_info "$(store_counts)"

	printf '\nMANUAL — the live discriminator this script cannot run (runbook §10 rows 1-2):\n'
	printf '  with OpenCode running, open TWO different workspaces (two agent instances/locations)\n'
	printf '  and call the "memory" tool with mode:"list-shards" in BOTH. Each must answer\n'
	printf '  success:true, storagePath=~/.opencode-mem/data, 22 shards, current 1 / linked 20 /\n'
	printf '  orphaned 1. Pre-fix the second workspace answered engine_unavailable (issue 82).\n'
	printf '  Also: memory mode:"help" in both must report engine_version 0.1.0, protocol 1.0.0,\n'
	printf '  unavailable: [] — and one call with options.codemode:false must succeed.\n'
	printf '  Then the host endpoints: curl http://127.0.0.1:<host-port>/api/skill (contains\n'
	printf '  opencode-mem.memory-usage) and /api/plugin (state active, source.type local).\n'

	verify_summary
}

verify_summary() {
	local mode=${1:-verify}
	heading 'summary'
	printf '  failures=%s warnings=%s\n' "$FAILS" "$WARNINGS"
	if [ "$FAILS" -ne 0 ]; then
		printf '  fix the FAIL lines; the runbook has the per-check expectation (§10) and the recovery (§9).\n'
	elif [ "$mode" = verify ]; then
		printf '  the engine-side battery is green; the two-workspace check above is still yours to run.\n'
	else
		printf '  the rollback completed; start OpenCode and confirm the upstream plugin writes again (§12).\n'
	fi
}

resolve_restore_ts() {
	local chosen=$FROM_TS
	if [ -z "$chosen" ]; then
		if [ -f "$BACKUP_ROOT/LATEST" ]; then
			chosen=$(tr -d '\n' <"$BACKUP_ROOT/LATEST")
		else
			local newest='' d=''
			for d in $(find "$BACKUP_ROOT" -maxdepth 1 -type d -name '20*' | sort -r); do
				if [ -f "$d/MANIFEST.sha256" ] && [ -f "$d/BACKUP-INFO" ]; then
					newest=$d
					break
				fi
			done
			[ -n "$newest" ] || die "no verified backup found under $BACKUP_ROOT (§12)"
			chosen=$(basename -- "$newest")
			log "no LATEST pointer; using the newest dated backup: $chosen"
		fi
	fi
	# LATEST is a file on disk too, so validate whichever source supplied the
	# stamp: only a dated directory name may become a restore path (§12).
	valid_restore_stamp "$chosen" || die "not a backup stamp: $chosen — refusing to restore from it (§12)"
	[ -d "$BACKUP_ROOT/$chosen" ] || die "backup not found: $BACKUP_ROOT/$chosen"
	printf '%s' "$chosen"
}

run_rollback_mode() {
	require_store
	require_config
	require_stopped
	begin_writing_log
	local ts='' backup='' aside='' saved=''
	ts=$(resolve_restore_ts)
	backup=$BACKUP_ROOT/$ts
	[ -d "$backup/data" ] || die "the backup has no data/: $backup (§12)"
	[ -f "$backup/config/opencode.jsonc" ] || die "the backup has no config/opencode.jsonc: $backup"
	[ -f "$backup/config/opencode-mem.jsonc" ] || die "the backup has no config/opencode-mem.jsonc: $backup"

	# The config half of §12 restores this backup's opencode.jsonc. A backup
	# taken AFTER the swap already points at the port, so restoring it would
	# leave the plugin swapped — a rollback that reports success and reverts
	# nothing. Refuse and name the pre-swap candidates instead.
	local to="\"$PLUGIN_DIR\""
	if [ "$(count_active_entry "$backup/config/opencode.jsonc" "$to")" -ge 1 ]; then
		local candidates='' d=''
		for d in $(find "$BACKUP_ROOT" -maxdepth 1 -type d -name '20*' | sort -r); do
			if [ -f "$d/config/opencode.jsonc" ] &&
				[ "$(count_active_entry "$d/config/opencode.jsonc" "$to")" -eq 0 ]; then
				candidates="$candidates $(basename -- "$d")"
			fi
		done
		die "the backup $ts was taken AFTER the swap: its config/opencode.jsonc already points at $PLUGIN_DIR, so restoring it would not revert the plugin entry (§12). Pre-swap backups:${candidates:- none found} — pass --from <TS>"
	fi

	# §5: a restore point is a directory whose manifest verifies. diff -r below
	# compares the copy to the very source it came from (faithful copy, not a
	# valid restore point), so verify the manifest BEFORE anything is written.
	[ -f "$backup/MANIFEST.sha256" ] || die "the backup has no MANIFEST.sha256: $backup — not restoring (§5)"
	if ! (cd "$backup" && sha256sum -c MANIFEST.sha256 --quiet); then
		die "the backup does not verify (§5) — not restoring"
	fi
	log "sha256sum -c on $backup: 0 failed — the restore point verifies"

	heading "rollback to $ts (runbook §12)"
	printf 'this will:\n'
	printf '  1. restore %s/config/opencode.jsonc     -> %s/opencode.jsonc\n' "$backup" "$CONFIG_DIR"
	printf '  2. restore %s/config/opencode-mem.jsonc -> %s/opencode-mem.jsonc\n' "$backup" "$CONFIG_DIR"
	printf '  3. mv %s -> %s/data.port-%s   (kept, never deleted)\n' "$DATA_DIR" "$BACKUP_ROOT" "$ts"
	printf '  4. cp -a %s/data -> %s   then diff -r to prove it is byte-identical\n' "$backup" "$DATA_DIR"
	confirm "restore the store and both config files from $ts (the port-era store is moved aside, not deleted)"
	if [ "$CONFIRMED" -ne 1 ]; then
		log 'the owner declined; nothing was written'
		exit 130
	fi

	saved=$BACKUP_ROOT/pre-rollback-$ts
	unique_suffix=$(unique_dir "$saved")
	mkdir -p "$unique_suffix"
	cp -a "$CONFIG_DIR/opencode.jsonc" "$unique_suffix/opencode.jsonc" || die 'cannot save the current opencode.jsonc'
	cp -a "$CONFIG_DIR/opencode-mem.jsonc" "$unique_suffix/opencode-mem.jsonc" || die 'cannot save the current opencode-mem.jsonc'
	log "current configs saved to $unique_suffix (kept)"

	cp -a "$backup/config/opencode.jsonc" "$CONFIG_DIR/opencode.jsonc" || die 'cannot restore opencode.jsonc'
	cp -a "$backup/config/opencode-mem.jsonc" "$CONFIG_DIR/opencode-mem.jsonc" || die 'cannot restore opencode-mem.jsonc'
	log "restored both config files from $backup/config"

	aside=$(unique_dir "$BACKUP_ROOT/data.port-$ts")
	log "mv $DATA_DIR $aside   (the port-era store is kept for diagnosis)"
	mv "$DATA_DIR" "$aside" || die "cannot move the live store aside: $DATA_DIR"

	if ! cp -a "$backup/data" "$DATA_DIR"; then
		log "FATAL the restore failed; the previous store is at $aside — moving it back"
		find "$DATA_DIR" -depth -delete 2>/dev/null || true
		mv "$aside" "$DATA_DIR" || true
		die 'cp -a of the backup data failed; the previous store was moved back (§12)'
	fi
	log "cp -a $backup/data $DATA_DIR"

	heading 'restore verification'
	if diff -r -q "$backup/data" "$DATA_DIR"; then
		ck_ok 'the restored data/ is byte-identical to the backup'
	else
		ck_fail 'the restored data/ differs from the backup — inspect before starting OpenCode'
	fi
	if [ -d "$DATA_DIR/projects" ]; then
		ck_info "restored counts: $(store_counts)"
	fi

	heading 'next'
	printf 'start OpenCode; the upstream plugin (restored in opencode.jsonc) writes again.\n'
	printf 'the port-era store is at: %s   (keep it; delete it yourself once you are sure)\n' "$aside"
	printf 'the previous configs are at: %s\n' "$unique_suffix"
	printf 'to go back to the port: re-run %s --swap\n' "$SCRIPT_PATH"
	verify_summary rollback
}

unique_suffix=

# ── main ──────────────────────────────────────────────────────────────────
main() {
	parse_args "$@"
	resolve_paths

	case $MODE in
	dry-run) dry_run ;;
	backup) run_backup_mode ;;
	swap) run_swap_mode ;;
	verify) run_verify_mode ;;
	rollback) run_rollback_mode ;;
	*) die "unknown mode: $MODE" ;;
	esac

	if [ "$FAILS" -ne 0 ]; then
		log "finished mode=$MODE failures=$FAILS warnings=$WARNINGS"
		exit 3
	fi
	log "finished mode=$MODE failures=0 warnings=$WARNINGS"
	exit 0
}

main "$@"

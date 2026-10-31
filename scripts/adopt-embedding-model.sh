#!/usr/bin/env bash
# adopt-embedding-model.sh — the owner-facing executor for swapping the live
# engine's embedding model from Xenova/nomic-embed-text-v1 (fp32) to
# Xenova/multilingual-e5-base (q8), plus the per-scope re-embed that the model
# change forces.
#
# The runbook is docs/runbooks/adopt-embedding-model.md; this script is the
# same sequence with the dangerous edges made mechanical:
#
#   * it refuses to write the live store or config while OpenCode (or the
#     store's engine) is running, and prints every destructive step first;
#   * --provision is a writing mode too: the provisioner's default invocation
#     canonicalises the store's nomic tokenizer IN PLACE, so it takes the same
#     stop check as --swap and --rollback;
#   * it refuses an engine built before the cache root moved into the store:
#     that binary reads <tmpfs>/.opencode-mem-onnx and finds no model in
#     <storagePath>/.cache, so the re-embed fails AFTER the config edit;
#   * it never opens a live database: metadata.db / shard DBs are copied
#     ({db,-wal}) into a work dir and only the copy is queried, and it asserts
#     the live root gained no new path (a live -shm is the hazard);
#   * the backup is not re-implemented — scripts/live-swap.sh --backup owns the
#     checkpoint, the dated cp -a, the sha256 manifest, the per-DB
#     integrity_check and the LATEST pointer;
#   * the model cache is not re-implemented — scripts/provision-onnx-cache.sh
#     owns the copy and the tokenizer canonicalisation;
#   * a mid-run re-embed failure is reported as what it is: some scopes on the
#     old model, the store refusing reads for those with a typed
#     ShardModelMismatch, an exact resume command, and the engine rebuild that
#     resume needs.
#
# It never starts OpenCode, never edits the runbook, never runs a git command
# that writes, and never writes the live root unless the mode says so.
#
# Modes (a bare invocation is --dry-run):
#   --dry-run    Print the plan and run the read-only preflight. Safe while
#                OpenCode runs. Writes nothing outside its work dir.
#   --backup     Delegate to scripts/live-swap.sh --backup (checkpoint, dated
#                backup, manifest, integrity, LATEST).
#   --provision  Copy + canonicalise the target model into the engine's cache
#                root and verify the pinned weights sha256. Idempotent.
#   --swap       --backup, --provision, the in-place config edit, then one
#                re-embed per scope, stopping at the first failure.
#   --verify     After the owner starts OpenCode: live-swap's battery plus the
#                store identity, a Thai and an English query canary, and RSS.
#   --rollback   Restore the pre-swap config and (by default) the backed-up
#                store; or --config-only to flip back and re-embed with nomic.
#
# usage: adopt-embedding-model.sh [MODE] [OPTIONS] — see --help.
set -euo pipefail

SCRIPT_PATH=$(readlink -f -- "${BASH_SOURCE[0]}")
REPO_ROOT=$(cd -- "$(dirname -- "$SCRIPT_PATH")/.." && pwd -P)

# ── defaults ──────────────────────────────────────────────────────────────
STORE_ROOT=$HOME/.opencode-mem
CONFIG_DIR=$HOME/.config/opencode
BACKUP_ROOT=$HOME/.opencode-mem-backups
WORK_ROOT=$REPO_ROOT/.tmp/adopt-embedding-model
LIVE_SWAP=$REPO_ROOT/scripts/live-swap.sh
PROVISIONER=$REPO_ROOT/scripts/provision-onnx-cache.sh
ENGINE_NAME=opencode-agents-memory
ENGINE_BIN=
SOURCE_DIR=$REPO_ROOT/.tmp/scratch/e5-base-src

# The adoption target. 768 dimensions is what the store already holds, so no
# store migration happens; the preset (mean pooling + query:/passage:
# prefixes) comes from crates/mem-core/src/config/normalize.rs and must NOT be
# written into the config — an explicit key beats the preset.
TARGET_MODEL=Xenova/multilingual-e5-base
TARGET_DTYPE=q8
TARGET_WEIGHTS=onnx/model_quantized.onnx
TARGET_WEIGHTS_BYTES=278647662
TARGET_WEIGHTS_SHA256=df7a9a29309e3ad491e1783adf8baee710262cc06079c7cbab63c630277fac94

# The store's current model, for --dry-run and the rollback text.
BASELINE_MODEL=Xenova/nomic-embed-text-v1

MODE="dry-run"
ASSUME_YES=0
REHEARSE_ROOT=
FROM_TS=
CONFIG_ONLY=0
declare -a SCOPE_FILTER=()

FAILS=0
WARNINGS=0
PENDING=0
LOG_FILE=
CONFIRMED=0
BACKUP_TS=
CONFIG_COPY=
STAMP=
WORK_RUN=

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
	printf 'adopt-embedding-model: %s\n' "$*" >&2
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

is_under() {
	case $1/ in
	"$2"/*) return 0 ;;
	*) return 1 ;;
	esac
}

valid_stamp() { [[ $1 =~ ^[0-9]{8}T[0-9]{6}Z$ ]]; }

first_line() {
	local out=''
	out=$(grep -m1 -oE "$1" "$2" 2>/dev/null || true)
	if [ -z "$out" ]; then
		return 0
	fi
	printf '%s\n' "$out"
}

# active_config_value <file> <key> — the first string value of an ACTIVE JSONC
# key. The `^[[:space:]]*"` anchor is what makes this different from a plain
# grep: the live config carries commented alternatives ("// "embeddingDtype":
# "q8",") and reporting one of those as the live value would be a lie.
active_config_value() {
	local raw=''
	raw=$(grep -m1 -oE "^[[:space:]]*\"$2\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" "$1" 2>/dev/null || true)
	if [ -z "$raw" ]; then
		return 0
	fi
	printf '%s' "$raw" | sed -E 's/^[[:space:]]*"[^"]*"[[:space:]]*:[[:space:]]*"(.*)"$/\1/'
}

# active_config_number <file> <key> — the first numeric value of an active key.
active_config_number() {
	local raw=''
	raw=$(grep -m1 -oE "^[[:space:]]*\"$2\"[[:space:]]*:[[:space:]]*[0-9]+" "$1" 2>/dev/null || true)
	if [ -z "$raw" ]; then
		return 0
	fi
	printf '%s' "$raw" | sed -E 's/^[[:space:]]*"[^"]*"[[:space:]]*:[[:space:]]*([0-9]+)$/\1/'
}

# active_key_lines <file> <extended-regex> — the active (non-comment) lines
# whose trimmed text matches. A commented line starts with `//` and can never
# match `^[[:space:]]*"`.
active_key_lines() {
	grep -n -E "^[[:space:]]*$2" "$1" 2>/dev/null || true
}

free_bytes() {
	local target=$1 probe=$1
	while [ ! -d "$probe" ] && [ "$probe" != / ]; do
		probe=$(dirname -- "$probe")
	done
	df -B1 --output=avail "$probe" | sed -n '2p' | tr -d ' '
}

dir_bytes() {
	local dir=$1
	if [ -d "$dir" ]; then
		du -sb "$dir" | cut -f1
	else
		printf '0'
	fi
}

sha256_of() {
	local file=$1
	[ -f "$file" ] || return 0
	sha256sum -- "$file" | cut -d' ' -f1
}

# ── --help ────────────────────────────────────────────────────────────────
usage() {
	cat <<'HELP'
adopt-embedding-model.sh — swap the live engine to Xenova/multilingual-e5-base q8

  The runbook is docs/runbooks/adopt-embedding-model.md. This script is the
  same sequence, ordered the same way, with its safety rules made mechanical.

USAGE
  adopt-embedding-model.sh [MODE] [OPTIONS]

  A bare invocation (no MODE) is --dry-run: it prints the plan and the
  read-only preflight and writes nothing outside its work dir — safe while
  OpenCode runs. Run every mode from a terminal with OpenCode stopped except
  --dry-run and --verify.

  Linux only (GNU coreutils: stat -c, df --output, du -sb, find -delete,
  sort -z).

MODES
  --dry-run   Preflight only. Checks that OpenCode and the store's engine are
              not running, the live config values (active keys only), the
              target model's cache or --source directory, the dimension
              resolution (768 required: the store is 768-wide), free disk
              (2x weights + two store copies), and the scope count read from a
              COPY of metadata.db. Prints every command the real modes run.
              Exit 3 if a FAIL line appears.

  --backup    scripts/live-swap.sh --backup: wal_checkpoint, dated cp -a of
              the store root, both config files, sha256 manifest + verify,
              per-DB integrity_check, LATEST pointer. The backup root must be
              outside the store root; nothing is overwritten.

  --provision Copy the model into `<storagePath>/.cache/<model>` (the store, on
              disk — the same tree upstream uses) through
              scripts/provision-onnx-cache.sh (which canonicalises the
              tokenizer in the copy, never in --source), then verify the pinned
              weights sha256 and byte count and refuse on a mismatch.
              Idempotent, and no longer needed after a reboot: the cache is
              not in /tmp. A WRITING mode: its default invocation (source ==
              destination) canonicalises the store's nomic tokenizer in place,
              so it refuses while OpenCode or the store's engine runs.

  --swap      The cutover, in this order:
                1. refuse while OpenCode or the engine runs,
                2. refuse an engine built before the cache root moved into the
                   store (it reads /tmp and cannot find the model),
                3. read the scope list from a COPY of metadata.db (a copy that
                   cannot be read aborts here, before anything is written),
                4. --backup (one dated restore point),
                5. --provision,
                6. copy opencode-mem.jsonc to .pre-model-swap-<TS>, then set
                   embeddingModel and embeddingDtype IN PLACE: one string on
                   one line, plus one inserted line if no active
                   embeddingDtype exists. Comments, commas and layout are
                   untouched; embeddingPooling / embeddingDimensions are
                   refused if active (an explicit key beats the e5 preset);
                7. run `--mode=reembed-scope` once per tag, printing each exit
                   code; the first failure stops the run and prints the resume
                   command and the engine rebuild it needs. Already-migrated
                   scopes short-circuit.
              It does NOT start OpenCode: you do, then run --verify.
              One scope failing is not "almost fine": the scopes not yet
              re-embedded still hold the old model and the store answers reads
              for them with a typed ShardModelMismatch.

  --verify    After you start OpenCode. Runs scripts/live-swap.sh --verify
              (engine count and parent, web health, add + search round-trip,
              store versions), then adds the model-specific checks: the
              resolved cache root (must be under the store, not /tmp) with the
              model present, the store's resolved identity (model + dtype +
              dimensions + fingerprint) read from a COPY of one shard, a Thai
              and an English query canary, and the engine's RSS. Then prints
              the manual checks it cannot do. Exit 3 on any FAIL.

  --rollback  Refuses while OpenCode or the engine runs. Two routes, and the
              cost of each:
                (default) restore the store from the backup that --swap took:
                  minutes, byte-identical to the restore point, and every
                  memory written after that backup is gone (it is moved aside,
                  never deleted);
                (--config-only) restore only opencode-mem.jsonc and re-embed
                  every scope with nomic: keeps post-backup memories, costs a
                  full re-embed (minutes to tens of minutes; the brief's 40-80
                  min figure is unverified), and
                  until each scope finishes the store refuses reads for it.
              Both restore the config byte-identically from
              .pre-model-swap-<TS> and canonicalise the restored nomic
              tokenizer in place (the upstream cache is v1.0, which this
              engine's reader rejects), then print the re-provision command.
              A default --rollback needs --from when more than one
              adopt-<stamp>.info exists: a resumed --swap writes a second
              record whose restore point is the mid-migration store, so
              "newest" is not the record you want.

OPTIONS
  --yes                Do not ask; you have already read the plan.
  --source DIR         Model directory to provision from (default
                       <repo>/.tmp/scratch/e5-base-src). Read-only.
  --store DIR          Store root (default ~/.opencode-mem; data/ lives in it).
  --config-dir DIR     Config directory (default ~/.config/opencode).
  --backup-root DIR    Backup root (default ~/.opencode-mem-backups). Must be
                       outside the store root.
  --work DIR           Scratch work dir (default <repo>/.tmp/adopt-embedding-
                       model). Live DBs are copied here and only the copies are
                       queried. Must be outside the store root.
  --engine PATH        The engine binary for --mode=reembed-scope (default the
                       staged plugin/engine/<os>-<arch>/opencode-agents-memory).
  --model ID           Target model (default Xenova/multilingual-e5-base). Any
                       model whose dimensions are not 768 is refused: the store
                       is 768-wide and a re-embed cannot change that.
  --weights-sha256 HEX Override the pinned weights sha256 (default: the
                       measured e5-base q8 digest df7a9a29...). --provision
                       verifies the file it provisioned against it and refuses
                       on a mismatch. Override only for a different --source;
                       an override is printed in every report.
  --weights-bytes N    Override the pinned weights byte count (default
                       278647662). Same rule as --weights-sha256.
  --scope TAG          --swap: re-embed only this container tag (repeatable).
                       For a targeted repair or a rehearsal; the default is
                       every active scope.
  --config-only        --rollback: restore the config only and re-embed with
                       nomic instead of restoring the backed-up store.
  --from TS            --rollback: the adoption stamp printed by --swap
                       (20261031T180000Z). `/`, `..` and paths are rejected.
  --rehearse DIR       Operate on a scratch copy, never the live root:
                       DIR/opencode-mem, DIR/config, DIR/backups, DIR/work.
                       The live-OpenCode check is skipped because nothing live
                       is written; a process holding the *scratch* storage root
                       is still a refusal. DIR must not contain (or be inside)
                       the live store or the live config dir.
  -h, --help           This text.

SEQUENCE (the owner's run, OpenCode stopped)
  1. quit OpenCode
  2. adopt-embedding-model.sh --dry-run      read the plan and the checks
  3. rebuild + re-stage the engine           cd <repo> &&
                                               rm -rf plugin/engine/<os>-<arch>/web-ui/dist &&
                                               bash plugin/scripts/stage-engine.sh
                                             (the cache root is compiled in: a staged engine
                                             from before this change still reads the old /tmp
                                             path, and --swap refuses it)
  4. adopt-embedding-model.sh --swap --yes   backup, provision, config, re-embed
  5. start OpenCode
  6. adopt-embedding-model.sh --verify       identity, canaries, cache, RSS
  7. keep the backup for a week; --rollback if a rollback trigger fires

EXIT STATUS
  0   the mode did what it reports
  1   refused or aborted (nothing live was written, or the write was reverted)
  2   a command-line argument was rejected
  3   the mode ran and a FAIL line was reported
  64  a re-embed rejected its scope tag (invalid container tag)
  74  a scope's re-embed answered success:false — the run stopped there
  130 the owner answered no
HELP
}

# ── argument parsing ──────────────────────────────────────────────────────
parse_args() {
	while [ $# -gt 0 ]; do
		case $1 in
		--dry-run) MODE="dry-run" ;;
		--backup) MODE=backup ;;
		--provision) MODE=provision ;;
		--swap) MODE=swap ;;
		--verify) MODE=verify ;;
		--rollback) MODE=rollback ;;
		--yes) ASSUME_YES=1 ;;
		--config-only) CONFIG_ONLY=1 ;;
		--rehearse)
			REHEARSE_ROOT=${2:?--rehearse needs a directory}
			shift
			;;
		--source)
			SOURCE_DIR=${2:?--source needs a directory}
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
		--work)
			WORK_ROOT=${2:?--work needs a directory}
			shift
			;;
		--engine)
			ENGINE_BIN=${2:?--engine needs a path}
			shift
			;;
		--model)
			TARGET_MODEL=${2:?--model needs an id}
			shift
			;;
		--weights-sha256)
			TARGET_WEIGHTS_SHA256=${2:?--weights-sha256 needs a digest}
			shift
			;;
		--weights-bytes)
			TARGET_WEIGHTS_BYTES=${2:?--weights-bytes needs a byte count}
			shift
			;;
		--scope)
			if [ $# -lt 2 ] || [ -z "$2" ]; then
				printf 'adopt-embedding-model: --scope needs a container tag\n' >&2
				exit 2
			fi
			SCOPE_FILTER+=("$2")
			shift
			;;
		--from)
			if [ $# -lt 2 ] || [ -z "$2" ]; then
				printf 'adopt-embedding-model: --from needs an adoption stamp\n' >&2
				exit 2
			fi
			FROM_TS=$2
			if ! valid_stamp "$FROM_TS"; then
				printf 'adopt-embedding-model: --from must be a stamp like 20261031T180000Z (got: %s)\n' "$FROM_TS" >&2
				exit 2
			fi
			shift
			;;
		-h | --help)
			usage
			exit 0
			;;
		*)
			printf 'adopt-embedding-model: unknown argument: %s\n' "$1" >&2
			printf 'run adopt-embedding-model.sh --help\n' >&2
			exit 2
			;;
		esac
		shift
	done
}

# ── path resolution and fail-closed validation ────────────────────────────
DATA_DIR=
CONFIG_STORAGE=
CACHE_ROOT=
LIVE_STORE_REAL=

# The engine resolves the model cache as `<storagePath>/.cache`
# (crates/mem-bridge/src/composition.rs, `local_cache_root`) — the same tree
# upstream uses. `<storagePath>` is the JSONC key (default
# ~/.opencode-mem/data), NOT the engine's --storage-root; the two coincide on
# this host, and a divergence is reported rather than silently guessed.
resolve_storage_path() {
	local raw='' expanded='' tilde='~'
	if [ -f "$CONFIG_DIR/opencode-mem.jsonc" ]; then
		raw=$(active_config_value "$CONFIG_DIR/opencode-mem.jsonc" storagePath)
	fi
	[ -n "$raw" ] || raw=$tilde/.opencode-mem/data
	expanded=$raw
	case $raw in
	"$tilde") expanded=$HOME ;;
	"$tilde"/*) expanded=$HOME/${raw#"$tilde"/} ;;
	esac
	CONFIG_STORAGE=$(abspath "$expanded")
	CACHE_ROOT=$CONFIG_STORAGE/.cache
}

resolve_paths() {
	if [ "$(uname -s)" != Linux ]; then
		die "this executor targets Linux only (GNU coreutils; the runbook's host is Void Linux)"
	fi
	if [ -n "$REHEARSE_ROOT" ]; then
		[ -d "$REHEARSE_ROOT" ] || die "--rehearse: not a directory: $REHEARSE_ROOT"
		REHEARSE_ROOT=$(abspath "$REHEARSE_ROOT")
		STORE_ROOT=$REHEARSE_ROOT/opencode-mem
		CONFIG_DIR=$REHEARSE_ROOT/config
		BACKUP_ROOT=$REHEARSE_ROOT/backups
		WORK_ROOT=$REHEARSE_ROOT/work
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
	WORK_ROOT=$(abspath "$WORK_ROOT")
	SOURCE_DIR=$(abspath "$SOURCE_DIR")
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
	if is_under "$WORK_ROOT" "$STORE_ROOT"; then
		die "the work dir must live OUTSIDE the store root ($WORK_ROOT is under $STORE_ROOT)"
	fi
	if [ "$(id -u)" -eq 0 ] && [ -z "$REHEARSE_ROOT" ]; then
		die "refusing to run as root: the runbook is a no-sudo, own-user procedure"
	fi
	if [ -z "$ENGINE_BIN" ]; then
		ENGINE_BIN=$(default_engine_path)
	fi
	ENGINE_BIN=$(abspath "$ENGINE_BIN")
	resolve_storage_path
}

# engine_stage_dir — the directory stage-engine.sh writes into. A custom
# --engine (a build tree, a rehearsal fake) has no staged bundle beside it, so
# the rebuild command names the repo's staged package instead.
engine_stage_dir() {
	local dir=''
	dir=$(dirname -- "$ENGINE_BIN")
	case $ENGINE_BIN in
	"$REPO_ROOT"/plugin/engine/*) ;;
	*) dir=$(dirname -- "$(default_engine_path)") ;;
	esac
	printf '%s' "$dir"
}

default_engine_path() {
	local os='' arch='' machine=''
	case $(uname -s) in
	Linux) os=linux ;;
	*) os='' ;;
	esac
	machine=$(uname -m)
	case $machine in
	x86_64 | amd64) arch=x64 ;;
	aarch64 | arm64) arch=arm64 ;;
	*) arch='' ;;
	esac
	if [ -z "$os" ] || [ -z "$arch" ]; then
		printf '%s' "$REPO_ROOT/plugin/engine/unknown/$ENGINE_NAME"
		return 0
	fi
	printf '%s/plugin/engine/%s-%s/%s' "$REPO_ROOT" "$os" "$arch" "$ENGINE_NAME"
}

require_store() {
	[ -d "$STORE_ROOT" ] || die "store root not found: $STORE_ROOT"
	[ -d "$DATA_DIR" ] || die "store data directory not found: $DATA_DIR"
}

require_config() {
	[ -d "$CONFIG_DIR" ] || die "config directory not found: $CONFIG_DIR"
	[ -f "$CONFIG_DIR/opencode-mem.jsonc" ] || die "missing $CONFIG_DIR/opencode-mem.jsonc"
	[ -f "$CONFIG_DIR/opencode.jsonc" ] || die "missing $CONFIG_DIR/opencode.jsonc (live-swap --backup copies both)"
}

require_engine() {
	[ -x "$ENGINE_BIN" ] || die "engine binary not found or not executable: $ENGINE_BIN (stage it with plugin/scripts/stage-engine.sh, or pass --engine)"
}

# The cache root is compiled in (crates/mem-bridge/src/composition.rs,
# `local_cache_root` = <storagePath>/.cache). An engine built before that change
# still bakes the tmpfs path and reads <tmpdir>/.opencode-mem-onnx, so it finds
# no model even when <store>/.cache is fully provisioned — measured: the staged
# package answered rc 74 at scope 1 of 22 AFTER the config had been flipped,
# while the fresh build re-embedded the same scope from the same store (rc 0).
# The check is a single grep (8 ms) and runs before the first write.
require_engine_reads_store_cache() {
	[ -x "$ENGINE_BIN" ] || die "engine binary not found or not executable: $ENGINE_BIN (stage it with plugin/scripts/stage-engine.sh, or pass --engine)"
	if LC_ALL=C grep -q 'opencode-mem-onnx' "$ENGINE_BIN"; then
		die "$ENGINE_BIN is an engine from before the cache-root change: it resolves ${TMPDIR:-/tmp}/.opencode-mem-onnx and finds no model in $CACHE_ROOT. Rebuild and re-stage it FIRST, then re-run — nothing has been written: rm -rf $(engine_stage_dir)/web-ui/dist && bash $REPO_ROOT/plugin/scripts/stage-engine.sh"
	fi
}

# ── target resolution (the dimension guard) ───────────────────────────────
# Mirrors embedding_dimensions_for_model (crates/mem-core/src/config/
# normalize.rs). An unknown id is NOT defaulted to 768 here: the engine's `_ =>
# 768` fallback is why a wrong pick fails silently, and this executor's job is
# to refuse rather than to guess.
model_dimensions() {
	case $1 in
	Xenova/multilingual-e5-base | intfloat/multilingual-e5-base) printf '768' ;;
	Xenova/nomic-embed-text-v1 | Xenova/nomic-embed-text-v1-unsupervised | Xenova/nomic-embed-text-v1-ablated) printf '768' ;;
	Xenova/jina-embeddings-v2-base-en | Xenova/all-mpnet-base-v2 | Xenova/bge-base-en-v1.5) printf '768' ;;
	Xenova/all-MiniLM-L6-v2 | Xenova/all-MiniLM-L12-v2 | Xenova/bge-small-en-v1.5 | Xenova/gte-small) printf '384' ;;
	Xenova/bge-m3 | intfloat/multilingual-e5-large | intfloat/e5-large-v2) printf '1024' ;;
	*) printf '' ;;
	esac
}

resolve_target() {
	TARGET_DIMS=$(model_dimensions "$TARGET_MODEL")
	if [ -z "$TARGET_DIMS" ]; then
		die "cannot resolve the dimensions of $TARGET_MODEL — this executor only knows the models in embedding_dimensions_for_model; refusing to guess (the engine's unknown-model fallback is 768, which is exactly the silent case this guard exists for)"
	fi
	if [ "$TARGET_DIMS" != 768 ]; then
		die "$TARGET_MODEL is ${TARGET_DIMS}-dim but the store is 768-wide (metadata + 22 shards, all 768): a re-embed cannot change a shard's width, and the engine would force a migration. Refusing — pick a 768-dim model (Xenova/multilingual-e5-base is the pinned one)."
	fi
	if [ "$TARGET_MODEL" != "$TARGET_MODEL_PINNED" ]; then
		die "no pinned weights file and sha256 for $TARGET_MODEL — this executor ships exactly one adoption ($TARGET_MODEL_PINNED q8, $TARGET_WEIGHTS, $TARGET_WEIGHTS_SHA256). Refusing to provision an unverified model."
	fi
}
TARGET_MODEL_PINNED=Xenova/multilingual-e5-base
TARGET_DIMS=

# ── process scan (nothing here writes) ────────────────────────────────────
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
			# A shell-script engine (a rehearsal fake, or a wrapper) appears as
			# `bash /path/<name> ...`; classify it the same way. The host probe
			# stays limited to the first two tokens so `bash -c '... opencode ...'`
			# cannot impersonate it.
			for token in "${args[@]:0:2}"; do
				if [ "${token##*/}" = "$ENGINE_NAME" ]; then
					role=engine
					break
				fi
				if [ "${token##*/}" = opencode ]; then
					role=host
					break
				fi
			done
		fi
		[ -n "$role" ] || continue
		for token in "${args[@]}"; do
			case $token in
			*adopt-embedding-model.sh* | *live-swap.sh*)
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

# require_stopped is the one guard every writing mode calls before its first
# write. A live pid in .engine.lock counts even when /proc has no matching
# cmdline: the lock is the store's own statement that it is owned.
require_stopped() {
	local engines='' hosts='' lock_pid=''
	refresh_procs
	engines=$(engine_pids_for_store | tr '\n' ' ' | sed -e 's/ *$//')
	hosts=$(host_pids)
	lock_pid=$(engine_lock_pid)
	if [ -n "$engines" ]; then
		die "an engine is still running for $DATA_DIR (pid ${engines}) — quit OpenCode and wait for it to exit"
	fi
	if [ -n "$lock_pid" ] && pid_alive "$lock_pid"; then
		die "$DATA_DIR/.engine.lock names a live pid ${lock_pid} — the store is still owned"
	fi
	if [ -n "$REHEARSE_ROOT" ]; then
		log "rehearsal: the live OpenCode process check is skipped (scratch store: $STORE_ROOT)"
		if [ -n "$hosts" ]; then
			log "rehearsal note: OpenCode is running (pid ${hosts}); nothing live is written"
		fi
		return 0
	fi
	if [ -n "$hosts" ]; then
		die "OpenCode is running (pid ${hosts}) — quit the application, then re-run"
	fi
	log "no OpenCode process and no engine for $DATA_DIR"
}

# ── live-root safety: copies only, and prove the copy added nothing ───────
live_paths_snapshot() {
	if [ -d "$DATA_DIR" ]; then
		find "$DATA_DIR" -mindepth 1 -maxdepth 2 -print | sort
	fi
}

# copy_db <abs-db> <dest-dir> — cp -a the database and its -wal into the work
# dir and echo the copy's path. The live file is never opened; a -shm is never
# copied (it is derived and would be rebuilt in the copy anyway).
copy_db() {
	local src=$1 dest=$2 base
	base=$(basename -- "$src")
	mkdir -p "$dest" || return 1
	cp -a -- "$src" "$dest/$base" || return 1
	if [ -f "$src-wal" ]; then
		cp -a -- "$src-wal" "$dest/$base-wal" || return 1
	fi
	printf '%s/%s\n' "$dest" "$base"
}

# assert_no_new_live_paths <before-snapshot> <strict:0|1>
# The ban is absolute for the derived sidecars: a new *-shm / *-wal / *-tshm
# means something opened a live database. Paths under <store>/data/.cache are
# exempt because that is where the model cache now lives by design (upstream
# parity: `<storagePath>/.cache`), and provisioning writes there.
assert_no_new_live_paths() {
	local before=$1 strict=$2 after='' new='' hazard=''
	after=$(live_paths_snapshot)
	new=$(comm -13 <(printf '%s\n' "$before") <(printf '%s\n' "$after") | sed '/^$/d')
	if [ -z "$new" ]; then
		return 0
	fi
	hazard=$(printf '%s\n' "$new" | grep -E -- '-(shm|wal|tshm)$' || true)
	if [ -n "$hazard" ]; then
		die "the live store root gained derived sidecar(s): $(printf '%s' "$hazard" | tr '\n' ' ') — a live DB was opened; stopping"
	fi
	new=$(printf '%s\n' "$new" | grep -vF -- "$DATA_DIR/.cache" || true)
	if [ -z "$new" ]; then
		return 0
	fi
	if [ "$strict" -eq 1 ]; then
		die "the live store root gained new path(s) while this script was only copying: $(printf '%s' "$new" | tr '\n' ' ') — stopping"
	fi
	ck_warn "the live store root gained path(s) during the read: $(printf '%s' "$new" | tr '\n' ' ') (the running engine's own writes; this script opened only the copies)"
	return 0
}

# ── store reads (all on copies) ───────────────────────────────────────────
make_work_dir() {
	local dry=$1
	if [ "$dry" -eq 1 ]; then
		mkdir -p "$(dirname -- "$WORK_ROOT")" || die "cannot create $(dirname -- "$WORK_ROOT")"
		WORK_RUN=$(mktemp -d "${WORK_ROOT}.dry.XXXXXX") || die "cannot create a scratch work dir under $(dirname -- "$WORK_ROOT")"
		trap 'find "$WORK_RUN" -depth -delete 2>/dev/null || true' EXIT
	else
		STAMP=${STAMP:-$(utc_stamp)}
		WORK_RUN=$WORK_ROOT/run-$STAMP
		mkdir -p "$WORK_RUN" || die "cannot create the work dir: $WORK_RUN"
	fi
}

container_tag_prefix() {
	local prefix=''
	if [ -f "$CONFIG_DIR/opencode-mem.jsonc" ]; then
		prefix=$(active_config_value "$CONFIG_DIR/opencode-mem.jsonc" containerTagPrefix)
	fi
	printf '%s' "${prefix:-opencode}"
}

# enumerate_scopes — one container tag per active scope, read from a COPY of
# metadata.db. stdout is tags only (the caller logs the count).
enumerate_scopes() {
	local dir=$WORK_RUN/metadata copy='' prefix=''
	[ -d "$DATA_DIR" ] || return 1
	[ -f "$DATA_DIR/metadata.db" ] || return 1
	copy=$(copy_db "$DATA_DIR/metadata.db" "$dir") || return 1
	prefix=$(container_tag_prefix)
	sqlite3 "$copy" \
		"SELECT DISTINCT '${prefix}_' || scope || '_' || scope_hash FROM shards WHERE is_active = 1 ORDER BY scope_hash;" 2>/dev/null
}

# scope_shard_db <tag> — the live absolute path of that scope's active shard,
# from the metadata copy. The stored `db_path` is re-anchored by BASENAME under
# `<scope>s/`, exactly as the engine's own reader does
# (ShardManager::resolve_stored_path): the registry writer emits
# `project/<file>` while the file lives in `projects/<file>` (measured: a
# re-embed rewrote 22 rows from `projects/…` to `project/…` and the file did
# not move), so joining the row verbatim under the data dir points at nothing.
scope_shard_db() {
	local tag=$1 dir=$WORK_RUN/metadata copy='' row='' scope='' rel='' base=''
	copy=$dir/metadata.db
	[ -f "$copy" ] || return 1
	row=$(sqlite3 "$copy" "SELECT scope || '|' || db_path FROM shards WHERE '${tag}' = '$(container_tag_prefix)_' || scope || '_' || scope_hash AND is_active = 1 LIMIT 1;" 2>/dev/null)
	[ -n "$row" ] || return 1
	scope=${row%%|*}
	rel=${row#*|}
	base=${rel##*/}
	[ -n "$base" ] || return 1
	printf '%s/%ss/%s' "$DATA_DIR" "$scope" "$base"
}

# shard_identity <copied-shard-db> — model, dimensions, fingerprint.
shard_identity() {
	local db=$1 model='' dims='' fingerprint=''
	model=$(sqlite3 "$db" "SELECT value FROM shard_metadata WHERE key = 'embedding_model' LIMIT 1;" 2>/dev/null || true)
	dims=$(sqlite3 "$db" "SELECT value FROM shard_metadata WHERE key = 'embedding_dimensions' LIMIT 1;" 2>/dev/null || true)
	fingerprint=$(sqlite3 "$db" "SELECT value FROM shard_metadata WHERE key = 'embedding_fingerprint' LIMIT 1;" 2>/dev/null || true)
	printf 'model=%s\ndims=%s\nfingerprint=%s\n' "${model:-<absent>}" "${dims:-<absent>}" "${fingerprint:-<absent>}"
}

tokenizer_version_of() {
	local file=$1 raw=''
	[ -f "$file" ] || return 0
	raw=$(first_line '"version"[[:space:]]*:[[:space:]]*"[0-9][0-9.]*"' "$file")
	[ -n "$raw" ] || return 0
	printf '%s' "$raw" | sed -E 's/.*"([0-9][0-9.]*)"$/\1/'
}

cache_root() { printf '%s' "$CACHE_ROOT"; }
cache_model_dir() { printf '%s/%s' "$CACHE_ROOT" "$TARGET_MODEL"; }
# The pre-adoption model's directory: what a rollback restores and has to
# canonicalise (cache_model_dir names the e5 target and is the wrong directory
# to report after a rollback).
baseline_cache_model_dir() { printf '%s/%s' "$CACHE_ROOT" "$BASELINE_MODEL"; }

# ── --backup: delegate, do not re-implement ───────────────────────────────
make_backup() {
	local out='' rc=0
	local -a args=(--backup --store "$STORE_ROOT" --config-dir "$CONFIG_DIR" --backup-root "$BACKUP_ROOT" --yes)
	if [ -n "$REHEARSE_ROOT" ]; then
		args+=(--rehearse "$REHEARSE_ROOT")
	fi
	heading 'backup (delegated to scripts/live-swap.sh --backup)'
	[ -x "$LIVE_SWAP" ] || [ -f "$LIVE_SWAP" ] || die "live-swap not found: $LIVE_SWAP"
	log "running $LIVE_SWAP ${args[*]}"
	out=$("$LIVE_SWAP" "${args[@]}" 2>&1) || rc=$?
	if [ "$rc" -ne 0 ]; then
		printf '%s\n' "$out" | tail -n 40 >&2
		die "the delegated backup failed (rc $rc) — nothing was provisioned or edited"
	fi
	printf '%s\n' "$out" | tail -n 8
	[ -f "$BACKUP_ROOT/LATEST" ] || die "the backup did not write $BACKUP_ROOT/LATEST — no restore point"
	BACKUP_TS=$(tr -d '\n' <"$BACKUP_ROOT/LATEST")
	valid_stamp "$BACKUP_TS" || die "LATEST is not a backup stamp: $BACKUP_TS"
	[ -d "$BACKUP_ROOT/$BACKUP_TS" ] || die "LATEST names a missing backup: $BACKUP_ROOT/$BACKUP_TS"
	ck_ok "restore point: $BACKUP_ROOT/$BACKUP_TS"
	log "backup stamp = $BACKUP_TS"
}

# ── --provision ───────────────────────────────────────────────────────────
provision() {
	local out='' rc=0 sha='' bytes='' dest='' tok=''
	heading 'provision the ONNX cache (scripts/provision-onnx-cache.sh)'
	[ -d "$SOURCE_DIR" ] || die "source model dir not found: $SOURCE_DIR (pass --source; the staging dir is <repo>/.tmp/scratch/e5-base-src)"
	[ -f "$SOURCE_DIR/$TARGET_WEIGHTS" ] || die "source has no $TARGET_WEIGHTS: $SOURCE_DIR"
	log "running $PROVISIONER --model $TARGET_MODEL --weights $TARGET_WEIGHTS --source $SOURCE_DIR --cache-root $CACHE_ROOT"
	out=$(sh "$PROVISIONER" --model "$TARGET_MODEL" --weights "$TARGET_WEIGHTS" \
		--source "$SOURCE_DIR" --cache-root "$CACHE_ROOT" 2>&1) || rc=$?
	if [ "$rc" -ne 0 ]; then
		printf '%s\n' "$out" | tail -n 40 >&2
		die "the ONNX provisioner failed (rc $rc) — search/add would answer -32603"
	fi
	printf '%s\n' "$out" | tail -n 6
	case $out in
	*tokenizer_version=2.0*) ck_ok 'the provisioner reports tokenizer_version=2.0' ;;
	*) die "the provisioner did not report tokenizer_version=2.0" ;;
	esac
	dest=$(printf '%s\n' "$out" | sed -n 's/^model_dir=//p')
	[ -n "$dest" ] || die "the provisioner did not report model_dir"
	tok=$(tokenizer_version_of "$dest/tokenizer.json")
	[ "$tok" = 2.0 ] || die "the provisioned tokenizer at $dest/tokenizer.json reads version ${tok:-unknown}, not 2.0"
	sha=$(printf '%s\n' "$out" | sed -n 's/^model_onnx_sha256=//p')
	bytes=$(printf '%s\n' "$out" | sed -n 's/^model_onnx_bytes=//p')
	if [ "$sha" != "$TARGET_WEIGHTS_SHA256" ]; then
		die "the provisioned $TARGET_WEIGHTS has sha256 $sha, not the pinned $TARGET_WEIGHTS_SHA256 — refusing (a partial or wrong download)"
	fi
	if [ "$bytes" != "$TARGET_WEIGHTS_BYTES" ]; then
		die "the provisioned $TARGET_WEIGHTS is $bytes B, not the pinned $TARGET_WEIGHTS_BYTES B — refusing"
	fi
	ck_ok "weights sha256 matches the pin ($TARGET_WEIGHTS_BYTES B)"
	printf 'cache_root=%s\n' "$(cache_root)"
	printf 'model=%s\n' "$TARGET_MODEL"
	printf 'model_dir=%s\n' "$dest"
	printf 'tokenizer_version=%s\n' "$tok"
	printf 'weights_file=%s\n' "$TARGET_WEIGHTS"
	printf 'weights_bytes=%s\n' "$bytes"
	printf 'weights_sha256=%s\n' "$sha"
	log "provisioned $TARGET_MODEL q8 at $dest"
}

# ── the config edit ───────────────────────────────────────────────────────
# edit_config <stamp> — set embeddingModel and embeddingDtype in place. The
# edit is line-local and literal: one string replaced on the active
# embeddingModel line, and either one string replaced on the active
# embeddingDtype line or one new line inserted after the model line. Comments,
# commas and layout are never touched, and the diff is verified to be exactly
# that.
edit_config() {
	local stamp=$1
	local config=$CONFIG_DIR/opencode-mem.jsonc
	local pre=$config.pre-model-swap-$stamp
	local model_hits='' model_lineno='' model_content='' indent=''
	local dtype_hits='' dtype_lineno='' bad='' tmp='' tmp2=''
	local old_model='' removed='' added='' expected_removed=1 expected_added=0 inserted=0
	local before_sha='' after_sha=''

	heading 'config edit (opencode-mem.jsonc, in place)'
	[ -f "$config" ] || die "missing $config"

	# Refusals first: an explicit key beats the multilingual-e5 preset
	# (config/mod.rs:253-274), and a remote embedding endpoint bypasses the
	# local lane entirely — both would make this adoption a no-op that reports
	# success.
	bad=$(active_key_lines "$config" '"embedding(Pooling|Dimensions|_dimensions)"[[:space:]]*:')
	if [ -n "$bad" ]; then
		die "an explicit embeddingPooling/embeddingDimensions key is active in $config:
$bad
An explicit key beats the multilingual-e5 preset, so this adoption would silently run with the wrong pooling or width. Uncomment nothing, remove the active key, and re-run."
	fi
	bad=$(active_key_lines "$config" '"embedding(ApiUrl|ApiKey)"[[:space:]]*:')
	if [ -n "$bad" ]; then
		die "a remote embedding endpoint is active in $config:
$bad
With embeddingApiUrl + embeddingApiKey set, the engine never loads a local ONNX model and changing embeddingModel renames the remote model instead. Refusing."
	fi

	model_hits=$(active_key_lines "$config" '"embeddingModel"[[:space:]]*:')
	if [ -z "$model_hits" ]; then
		die "no active \"embeddingModel\" key in $config — not the file this executor edits"
	fi
	if [ "$(printf '%s\n' "$model_hits" | sed '/^$/d' | wc -l | tr -d ' ')" -ne 1 ]; then
		die "$config has more than one active \"embeddingModel\" key — ambiguous, refusing to guess"
	fi
	model_lineno=${model_hits%%:*}
	model_content=$(sed -n "${model_lineno}p" "$config")
	indent=${model_content%%\"*}
	old_model=$(active_config_value "$config" embeddingModel)
	[ -n "$old_model" ] || die "the active \"embeddingModel\" line has no string value: $model_content"

	if [ "$old_model" = "$TARGET_MODEL" ] && [ "$(active_config_value "$config" embeddingDtype)" = "$TARGET_DTYPE" ]; then
		log "config already reads embeddingModel=$TARGET_MODEL embeddingDtype=$TARGET_DTYPE — nothing to change (idempotent)"
		ck_info 'config already at the target; no new .pre-model-swap copy'
		CONFIG_COPY=$(newest_config_copy || true)
		if [ -n "$CONFIG_COPY" ]; then
			ck_info "the restore copy is the newest existing one: $CONFIG_COPY"
		else
			ck_warn 'no .pre-model-swap-<TS> copy exists — --rollback will have to restore opencode-mem.jsonc from the backup'
		fi
		return 0
	fi

	printf '\nplanned edit of %s:\n' "$config"
	printf '  - "embeddingModel": "%s"\n' "$old_model"
	printf '  + "embeddingModel": "%s"\n' "$TARGET_MODEL"
	dtype_hits=$(active_key_lines "$config" '"embeddingDtype"[[:space:]]*:')
	if [ -n "$dtype_hits" ]; then
		printf '  - "embeddingDtype": "%s"\n' "$(active_config_value "$config" embeddingDtype)"
		printf '  + "embeddingDtype": "%s"\n' "$TARGET_DTYPE"
		expected_added=2
		expected_removed=2
	else
		printf '  + %s"embeddingDtype": "%s",   (one new line after the model line)\n' "$indent" "$TARGET_DTYPE"
		expected_added=2
		inserted=1
	fi
	printf '  (comments, commas and every other line untouched; no embeddingPooling, no embeddingDimensions)\n'
	confirm "write these two keys into $config (copy kept as $(basename -- "$pre"))"
	if [ "$CONFIRMED" -ne 1 ]; then
		log 'the owner declined the config edit; the backup above is intact'
		exit 130
	fi

	before_sha=$(sha256_of "$config")
	cp -a "$config" "$pre" || die "cannot write the pre-swap copy: $pre"
	if [ "$(sha256_of "$pre")" != "$before_sha" ]; then
		die "the pre-swap copy does not match the config — nothing edited"
	fi
	CONFIG_COPY=$pre
	log "config copy: $pre"

	tmp=$(mktemp "$CONFIG_DIR/.opencode-mem.jsonc.new.XXXXXX") || die "cannot create a temp file in $CONFIG_DIR"
	trap 'find "$CONFIG_DIR" -maxdepth 1 -name ".opencode-mem.jsonc.new.*" -delete 2>/dev/null || true' EXIT
	awk -v n="$model_lineno" -v old="\"$old_model\"" -v new="\"$TARGET_MODEL\"" '
		NR == n {
			i = index($0, old)
			if (i > 0) { $0 = substr($0, 1, i - 1) new substr($0, i + length(old)) }
		}
		{ print }
	' "$config" >"$tmp" || die "the model replacement failed; nothing edited"

	if [ "$inserted" -eq 1 ]; then
		tmp2=$(mktemp "$CONFIG_DIR/.opencode-mem.jsonc.new.XXXXXX") || die "cannot create a temp file in $CONFIG_DIR"
		awk -v n="$model_lineno" -v line="${indent}\"embeddingDtype\": \"$TARGET_DTYPE\"," '
			NR == n { print; print line; next }
			{ print }
		' "$tmp" >"$tmp2" || die "the dtype insertion failed; nothing edited"
		mv -f "$tmp2" "$tmp"
	else
		dtype_lineno=${dtype_hits%%:*}
		local old_dtype=''
		old_dtype=$(active_config_value "$config" embeddingDtype)
		tmp2=$(mktemp "$CONFIG_DIR/.opencode-mem.jsonc.new.XXXXXX") || die "cannot create a temp file in $CONFIG_DIR"
		awk -v n="$dtype_lineno" -v old="\"$old_dtype\"" -v new="\"$TARGET_DTYPE\"" '
			NR == n {
				i = index($0, old)
				if (i > 0) { $0 = substr($0, 1, i - 1) new substr($0, i + length(old)) }
			}
			{ print }
		' "$tmp" >"$tmp2" || die "the dtype replacement failed; nothing edited"
		mv -f "$tmp2" "$tmp"
	fi

	# Verify the result before it replaces the live file: exactly one active
	# model line carrying the target, the old value gone from active lines,
	# exactly one active dtype line, and a diff of 1 removed / 1-or-2 added.
	if [ "$(active_config_value "$tmp" embeddingModel)" != "$TARGET_MODEL" ]; then
		die "the edited file does not carry embeddingModel=$TARGET_MODEL; nothing edited"
	fi
	if [ "$(active_config_value "$tmp" embeddingDtype)" != "$TARGET_DTYPE" ]; then
		die "the edited file does not carry embeddingDtype=$TARGET_DTYPE; nothing edited"
	fi
	if [ "$(active_key_lines "$tmp" '"embeddingModel"[[:space:]]*:' | sed '/^$/d' | wc -l | tr -d ' ')" -ne 1 ]; then
		die "the edit produced more than one active embeddingModel key; nothing edited"
	fi
	if [ "$(active_key_lines "$tmp" '"embeddingDtype"[[:space:]]*:' | sed '/^$/d' | wc -l | tr -d ' ')" -ne 1 ]; then
		die "the edit produced more than one active embeddingDtype key; nothing edited"
	fi
	removed=$( { diff --unchanged-line-format='' --old-line-format='-%L' --new-line-format='' "$config" "$tmp" || true; } | wc -l | tr -d ' ')
	added=$( { diff --unchanged-line-format='' --old-line-format='' --new-line-format='+%L' "$config" "$tmp" || true; } | wc -l | tr -d ' ')
	if [ "$removed" -ne "$expected_removed" ] || [ "$added" -ne "$expected_added" ]; then
		die "the edit would change $removed line(s) and add $added (expected $expected_removed / $expected_added) — refusing; nothing edited"
	fi
	# Comments are pinned by the counts above: a comment edit would change the
	# removed/added line counts, so no separate "the comment survived" check is
	# needed (the one that used to sit here could only ever warn).

	chmod "$(stat -c %a "$config")" "$tmp"
	mv -f "$tmp" "$config" || die "cannot replace $config"
	trap - EXIT
	after_sha=$(sha256_of "$config")
	log "edited $config (sha256 $before_sha -> $after_sha)"
	printf '\n--- diff (pre-swap -> current) ---\n'
	diff -u "$pre" "$config" || true
	printf '\nrestore command (any time):\n  cp -a %s %s\n' "$pre" "$config"
	printf 'backup (whole store + both configs):\n  %s\n' "${BACKUP_ROOT}/${BACKUP_TS:-<TS>}"
}

newest_config_copy() {
	find "$CONFIG_DIR" -maxdepth 1 -type f -name 'opencode-mem.jsonc.pre-model-swap-*' 2>/dev/null | sort | tail -n 1
}

# ── the per-scope re-embed ────────────────────────────────────────────────
in_scope_filter() {
	local tag=$1 candidate=''
	if [ "${#SCOPE_FILTER[@]}" -eq 0 ]; then
		return 0
	fi
	for candidate in "${SCOPE_FILTER[@]}"; do
		if [ "$candidate" = "$tag" ]; then
			return 0
		fi
	done
	return 1
}

# run_reembed_scope <tag> — one scope, one process, its exit code reported.
# Returns 0 or the engine's exit code (64 invalid tag, 74 success:false).
run_reembed_scope() {
	local tag=$1 rc=0 out='' json='' success='' re_embedded=''
	local child_home=$HOME
	log "re-embedding $tag"
	# The engine loads its config from $HOME/.config/opencode. A rehearsal
	# config lives under the scratch root, so the child gets a HOME whose
	# .config/opencode is that config — otherwise the rehearsal would re-embed
	# against the LIVE config (a different model, and a no-op short-circuit).
	if [ -n "$REHEARSE_ROOT" ]; then
		child_home=$REHEARSE_ROOT/home
		mkdir -p "$child_home/.config/opencode" || die "cannot create $child_home/.config/opencode"
		cp -a "$CONFIG_DIR/opencode-mem.jsonc" "$child_home/.config/opencode/opencode-mem.jsonc" ||
			die "cannot mirror the scratch config into $child_home"
	fi
	out=$(HOME="$child_home" "$ENGINE_BIN" --mode=reembed-scope --scope "$tag" --storage-root "$DATA_DIR" 2>&1) || rc=$?
	json=$(printf '%s\n' "$out" | grep -m1 '"event":"reembed.completed"' || true)
	if [ -n "$json" ]; then
		success=$(printf '%s' "$json" | jq -r '.success // "unknown"' 2>/dev/null || printf 'unknown')
		re_embedded=$(printf '%s' "$json" | jq -r '.re_embedded_memories // 0' 2>/dev/null || printf '0')
	else
		success=unknown
		re_embedded=0
	fi
	printf '  [ rc ] %s rc=%s success=%s re_embedded=%s\n' "$tag" "$rc" "$success" "$re_embedded"
	if [ -n "$json" ]; then
		printf '  %s\n' "$json"
	fi
	if [ "$rc" -ne 0 ]; then
		printf '%s\n' "$out" | grep -v '"event":"reembed.completed"' | tail -n 8 | sed 's/^/       /' || true
	fi
	if [ "$rc" -eq 0 ]; then
		log "re-embedded $tag (re_embedded=$re_embedded)"
		return 0
	fi
	log "re-embed FAILED for $tag (rc $rc)"
	return "$rc"
}

# print_reembed_resume <failed-tag> <migrated-count> [remaining...]
# The migrated count is what makes the first sentence true: at the measured
# failure (a stale engine at scope 1 of 22) ZERO scopes were on the new model
# and the block claimed "the scopes already listed above are on e5".
print_reembed_resume() {
	local failed=$1 migrated=$2
	shift 2
	local -a remaining=("$@")
	local -a resume=(--swap --yes)
	local scope_tag=$failed
	printf '\n== STOPPED ==\n'
	if [ "$migrated" -gt 0 ]; then
		printf 'scope %s failed. The %s scope(s) already listed above are on %s;\n' \
			"$failed" "$migrated" "$TARGET_MODEL"
		printf 'every scope not listed above still holds %s vectors. The store answers\n' "$BASELINE_MODEL"
	else
		printf 'scope %s failed and NO scope has been migrated — the store is unchanged\n' "$failed"
		printf '(every scope still holds %s vectors). The store answers\n' "$BASELINE_MODEL"
	fi
	printf 'reads for those scopes with a typed ShardModelMismatch — that is the designed refusal,\n'
	printf 'not corruption, and it is NOT "almost fine": search in those workspaces fails until\n'
	printf 'they are re-embedded.\n'
	if [ "${#remaining[@]}" -gt 0 ]; then
		printf 'not yet attempted (%s): %s\n' "${#remaining[@]}" "${remaining[*]}"
	fi
	if [ -n "$REHEARSE_ROOT" ]; then
		resume+=(--rehearse "$REHEARSE_ROOT")
	fi
	if [ "${#remaining[@]}" -gt 0 ]; then
		scope_tag=${remaining[0]}
	fi
	printf '\nSTEP 1 — rebuild + re-stage the engine (the cache root is compiled in; a stale\n'
	printf 'engine fails every scope the same way):\n'
	printf '  rm -rf %s/web-ui/dist && bash %s/plugin/scripts/stage-engine.sh\n' \
		"$(engine_stage_dir)" "$REPO_ROOT"
	printf '  LC_ALL=C grep -c opencode-mem-onnx %s   # must print 0\n' "$ENGINE_BIN"
	printf '\nSTEP 2 — resume (backup, provision and the config edit are idempotent; migrated\nscopes short-circuit without embedding anything):\n'
	printf '  %s %s\n' "$SCRIPT_PATH" "${resume[*]}"
	printf '  # or only the remaining tags, one at a time:\n'
	printf '  %s %s --scope %s\n' "$SCRIPT_PATH" "${resume[*]}" "$scope_tag"
}

# reembed_all <tags-file> — one re-embed per container tag, stopping at the
# first failure. The tag list is enumerated by run_swap_mode BEFORE its first
# write, so a metadata copy that cannot be read aborts before the backup and
# the config edit; a process substitution here (`< <(enumerate_scopes)`)
# discarded that status and turned a failed copy into "no active scopes".
reembed_all() {
	local tags_file=$1
	local -a tags=() remaining=() selected=()
	local tag='' i=0 rc=0 before=''
	heading 're-embed every scope (one process per container tag)'
	before=$(live_paths_snapshot)
	mapfile -t tags <"$tags_file"
	assert_no_new_live_paths "$before" 1
	if [ "${#tags[@]}" -eq 0 ]; then
		ck_warn 'no active shard rows in metadata.db — nothing to re-embed (is this the store root?)'
		return 0
	fi
	for tag in "${tags[@]}"; do
		if in_scope_filter "$tag"; then
			selected+=("$tag")
		fi
	done
	if [ "${#selected[@]}" -eq 0 ]; then
		die "no active scope matches --scope (${SCOPE_FILTER[*]}); active scopes: ${tags[*]}"
	fi
	log "re-embedding ${#selected[@]} of ${#tags[@]} scopes"
	for ((i = 0; i < ${#selected[@]}; i++)); do
		tag=${selected[i]}
		rc=0
		run_reembed_scope "$tag" || rc=$?
		if [ "$rc" -ne 0 ]; then
			remaining=("${selected[@]:i+1}")
			print_reembed_resume "$tag" "$i" "${remaining[@]}"
			final_summary "$rc"
		fi
	done
	ck_ok "all ${#selected[@]} selected scope(s) answered success:true"
}

# ── bookkeeping outside the store ─────────────────────────────────────────
write_adopt_info() {
	local info=$BACKUP_ROOT/adopt-$STAMP.info
	[ -n "$STAMP" ] || die 'internal: no adoption stamp'
	{
		printf 'stamp=%s\n' "$STAMP"
		printf 'backup_ts=%s\n' "$BACKUP_TS"
		printf 'config_copy=%s\n' "${CONFIG_COPY:-}"
		printf 'config=%s\n' "$CONFIG_DIR/opencode-mem.jsonc"
		printf 'config_sha256_after=%s\n' "$(sha256_of "$CONFIG_DIR/opencode-mem.jsonc")"
		printf 'model=%s\n' "$TARGET_MODEL"
		printf 'dtype=%s\n' "$TARGET_DTYPE"
		printf 'dimensions=%s\n' "$TARGET_DIMS"
		printf 'store_root=%s\n' "$STORE_ROOT"
		printf 'created=%s\n' "$(now_utc)"
	} >"$info" || die "cannot write $info"
	log "adoption record: $info"
}

final_summary() {
	local rc=${1:-0}
	local cache_dir='' cache_model=''
	local pre_files='' pre_bytes=''
	heading 'summary'
	printf '  backup path     : %s\n' "${BACKUP_ROOT}/${BACKUP_TS:-<none>}"
	printf '  config copy     : %s\n' "${CONFIG_COPY:-<none>}"
	printf '  rollback        : %s --rollback --yes --from %s\n' "$SCRIPT_PATH" "${STAMP:-<stamp>}"
	# After a rollback the engine reads the pre-adoption model again, so the e5
	# directory is not the cache to report (it was moved aside with the store).
	if [ "$MODE" = rollback ]; then
		cache_dir=$(baseline_cache_model_dir)
		cache_model=$BASELINE_MODEL
	else
		cache_dir=$(cache_model_dir)
		cache_model=$TARGET_MODEL
	fi
	if [ -f "$cache_dir/onnx/model_quantized.onnx" ] || [ -f "$cache_dir/onnx/model.onnx" ]; then
		printf '  model cache     : %s   (%s, under the store: disk-backed, survives a reboot)\n' "$cache_dir" "$cache_model"
	else
		printf '  model cache     : <absent> for %s — %s --provision\n' "$cache_model" "$SCRIPT_PATH"
	fi
	if [ -e "${TMPDIR:-/tmp}/.opencode-mem-onnx" ]; then
		printf '  reclaim the old tmpfs cache (the engine no longer reads it):\n'
		printf '    find %s/.opencode-mem-onnx -depth -delete\n' "${TMPDIR:-/tmp}"
	fi
	# The engine writes one .bak + .bak-wal per re-embedded scope INSIDE data/
	# (crates/mem-store). live-swap's checkpoint ignores them; they are the
	# owner's to delete once --verify passes.
	pre_files=$(find "$DATA_DIR" -type f -name '*pre-reembed*' | wc -l | tr -d ' ')
	if [ "$pre_files" -gt 0 ]; then
		pre_bytes=$(find "$DATA_DIR" -type f -name '*pre-reembed*' -printf '%s\n' | awk '{ total += $1 } END { printf "%d", total + 0 }')
		printf '  engine backups  : %s pre-reembed file(s), %s B under %s — one per re-embedded scope; delete them once --verify passes:\n' \
			"$pre_files" "$pre_bytes" "$DATA_DIR"
		printf '    find %s -name "*pre-reembed*" -depth -delete\n' "$DATA_DIR"
	fi
	printf '  log             : %s\n' "${LOG_FILE:-<stdout only>}"
	printf '\n  episodic.db: agent_sessions.embedding is NEVER re-embedded.\n'
	case $MODE:$CONFIG_ONLY in
	swap:*)
		printf '  Rows written before this adoption keep nomic-space vectors beside the new\n'
		printf '  e5-space ones (same 768 width, so nothing errors — the similarity is just\n'
		printf '  meaningless for those rows). Search over episodic consolidation is affected;\n'
		printf '  memory search over the project shards is not.\n'
		;;
	rollback:1)
		printf '  The config is back on nomic, but the rows written during the e5 window keep\n'
		printf '  e5-space vectors (same 768 width, so nothing errors — the similarity for\n'
		printf '  those rows is meaningless once the shards are re-embedded with nomic).\n'
		;;
	*)
		printf '  The restored store is the pre-adoption backup: it carries no e5 rows. Rows\n'
		printf '  written after that backup are in the moved-aside store, not in the live one.\n'
		;;
	esac
	if [ "$rc" -ne 0 ]; then
		printf '\n  the run stopped (rc %s) — see the STOPPED block above for what is and is not migrated.\n' "$rc"
		printf '  rebuild + re-stage the engine before resuming: the cache root is compiled in.\n'
		printf '    rm -rf %s/web-ui/dist && bash %s/plugin/scripts/stage-engine.sh\n' \
			"$(engine_stage_dir)" "$REPO_ROOT"
		exit "$rc"
	fi
	if [ "$FAILS" -ne 0 ]; then
		exit 3
	fi
}

# ── --dry-run ─────────────────────────────────────────────────────────────
dry_run() {
	heading 'adopt-embedding-model --dry-run (reads only; nothing outside the work dir is written)'
	log "store=$STORE_ROOT config=$CONFIG_DIR backups=$BACKUP_ROOT work=$WORK_ROOT"
	if [ -n "$REHEARSE_ROOT" ]; then
		log "rehearsal root=$REHEARSE_ROOT (scratch paths, not the live ones)"
	fi
	resolve_target
	ck_info "target: $TARGET_MODEL dtype=$TARGET_DTYPE dimensions=$TARGET_DIMS weights=$TARGET_WEIGHTS"

	heading '1. preflight'
	if [ -d "$STORE_ROOT" ] && [ -d "$DATA_DIR" ]; then
		ck_ok "store root exists: $STORE_ROOT"
		ck_info "store bytes=$(dir_bytes "$STORE_ROOT")"
	else
		ck_fail "store root/data not found: $STORE_ROOT (expected \$HOME/.opencode-mem/data) — pass --store to override"
	fi
	if [ -f "$CONFIG_DIR/opencode-mem.jsonc" ]; then
		ck_ok "config exists: $CONFIG_DIR/opencode-mem.jsonc"
	else
		ck_fail "missing $CONFIG_DIR/opencode-mem.jsonc"
	fi
	if [ -x "$ENGINE_BIN" ]; then
		ck_ok "engine: $ENGINE_BIN"
		ck_info "engine sha256=$(sha256_of "$ENGINE_BIN")"
		# Pending, never FAIL: the dry-run is step 2 of the runbook and the
		# rebuild is step 3, so a FAIL here would make the documented order
		# start red.
		if LC_ALL=C grep -q 'opencode-mem-onnx' "$ENGINE_BIN"; then
			ck_pending "the engine at $ENGINE_BIN predates the cache-root change (it resolves ${TMPDIR:-/tmp}/.opencode-mem-onnx) — rebuild + re-stage it BEFORE --swap: rm -rf $(engine_stage_dir)/web-ui/dist && bash $REPO_ROOT/plugin/scripts/stage-engine.sh"
		else
			ck_ok 'the engine carries the store cache root (no baked tmpfs path)'
		fi
	else
		ck_fail "no executable engine at $ENGINE_BIN (pass --engine, or stage it with plugin/scripts/stage-engine.sh)"
	fi

	heading '1b. live config values (active keys only)'
	if [ -f "$CONFIG_DIR/opencode-mem.jsonc" ]; then
		local config=$CONFIG_DIR/opencode-mem.jsonc
		local model='' dtype='' pooling='' dims_key='' api_url='' storage=''
		model=$(active_config_value "$config" embeddingModel)
		dtype=$(active_config_value "$config" embeddingDtype)
		pooling=$(active_config_value "$config" embeddingPooling)
		dims_key=$(active_config_number "$config" embeddingDimensions)
		api_url=$(active_config_value "$config" embeddingApiUrl)
		storage=$(active_config_value "$config" storagePath)
		ck_info "storagePath     = ${storage:-<unset>} (the plugin resolves \$HOME/.opencode-mem/data)"
		ck_info "embeddingModel  = ${model:-<unset, engine default $BASELINE_MODEL>}"
		ck_info "embeddingDtype  = ${dtype:-<unset>} (the adoption writes \"$TARGET_DTYPE\")"
		if [ -n "$pooling" ]; then
			ck_fail "embeddingPooling = $pooling is ACTIVE — an explicit key beats the multilingual-e5 preset; the edit refuses"
		else
			ck_ok 'no active embeddingPooling (the preset supplies mean pooling)'
		fi
		if [ -n "$dims_key" ]; then
			ck_fail "embeddingDimensions = $dims_key is ACTIVE — remove it; the preset/table must supply 768"
		else
			ck_ok 'no active embeddingDimensions (auto-detected: 768)'
		fi
		if [ -n "$api_url" ]; then
			ck_fail "embeddingApiUrl = $api_url is ACTIVE — the engine would use the remote lane and never load a local model"
		else
			ck_ok 'no active embeddingApiUrl (local ONNX lane)'
		fi
		if [ "$model" = "$TARGET_MODEL" ] && [ "$dtype" = "$TARGET_DTYPE" ]; then
			ck_ok 'the config already names the target model and dtype (the edit would be a no-op)'
		fi
	else
		ck_fail 'cannot read the config values'
	fi

	heading '1c. is anything running?'
	local engines='' hosts='' lock_pid=''
	refresh_procs
	engines=$(engine_pids_for_store | tr '\n' ' ' | sed -e 's/ *$//')
	hosts=$(host_pids)
	lock_pid=$(engine_lock_pid)
	if [ -n "$engines" ]; then
		ck_pending "engine running for this store: pid ${engines} — every writing mode refuses"
	else
		ck_ok 'no engine process for this store'
	fi
	if [ -n "$hosts" ] && [ -n "$REHEARSE_ROOT" ]; then
		ck_info "OpenCode running: pid ${hosts} — irrelevant to these scratch paths"
	elif [ -n "$hosts" ]; then
		ck_pending "OpenCode running: pid ${hosts} — quit it before --swap/--rollback"
	else
		ck_ok 'no OpenCode process'
	fi
	if [ -n "$lock_pid" ] && pid_alive "$lock_pid"; then
		ck_pending "the store lock names a live pid ${lock_pid} — writing modes refuse"
	fi

	heading '1d. the target model in the engine cache'
	local cache='' tok='' have=''
	cache=$(cache_model_dir)
	ck_info "cache root: $CACHE_ROOT (<storagePath>/.cache; the engine's resolved storagePath is $CONFIG_STORAGE)"
	if [ "$CONFIG_STORAGE" != "$DATA_DIR" ]; then
		ck_warn "the config's storagePath ($CONFIG_STORAGE) and the engine's store root ($DATA_DIR) differ — the cache follows the config key, the shards follow --storage-root"
	fi
	if is_under "$CACHE_ROOT" "$CONFIG_STORAGE"; then
		ck_ok 'the cache root is under the store (disk-backed, survives a reboot — not tmpfs)'
	else
		ck_fail "the cache root is NOT under $CONFIG_STORAGE"
	fi
	if [ -f "$cache/$TARGET_WEIGHTS" ]; then
		tok=$(tokenizer_version_of "$cache/tokenizer.json")
		have=$(wc -c <"$cache/$TARGET_WEIGHTS" | tr -d ' ')
		ck_info "cache: $cache (tokenizer_version=${tok:-unknown}, $TARGET_WEIGHTS=$have B)"
		if [ "$tok" = 2.0 ]; then
			ck_ok 'the cached tokenizer is the canonical v2.0 form'
		else
			ck_pending "the cached tokenizer is version ${tok:-unknown} — --provision rewrites it"
		fi
		if [ "$have" != "$TARGET_WEIGHTS_BYTES" ]; then
			ck_fail "the cached $TARGET_WEIGHTS is $have B, the pin is $TARGET_WEIGHTS_BYTES B — re-run --provision"
		fi
	else
		ck_pending "no $TARGET_WEIGHTS under $cache — --swap provisions it into the store"
	fi
	if [ -d "$SOURCE_DIR" ] && [ -f "$SOURCE_DIR/$TARGET_WEIGHTS" ]; then
		local src_bytes=''
		src_bytes=$(wc -c <"$SOURCE_DIR/$TARGET_WEIGHTS" | tr -d ' ')
		ck_ok "source: $SOURCE_DIR ($TARGET_WEIGHTS=$src_bytes B)"
		if [ "$src_bytes" != "$TARGET_WEIGHTS_BYTES" ]; then
			ck_fail "the source $TARGET_WEIGHTS is $src_bytes B, the pin is $TARGET_WEIGHTS_BYTES B"
		fi
		if [ -f "$SOURCE_DIR/${TARGET_WEIGHTS}_data" ]; then
			ck_fail "the source weights are a stub with a sibling ${TARGET_WEIGHTS}_data — the engine cannot read external-data ONNX"
		fi
		if [ "$(tokenizer_version_of "$SOURCE_DIR/tokenizer.json")" = 2.0 ]; then
			ck_info 'the source tokenizer already reads v2.0 (provisioning would skip canonicalisation)'
		else
			ck_info 'the source tokenizer is legacy (v1.0) — provisioning canonicalises the COPY (~35 s cold)'
		fi
	else
		ck_fail "no source at $SOURCE_DIR — pass --source DIR with $TARGET_WEIGHTS"
	fi

	heading '1e. disk'
	local store_bytes='' need_backup='' need_cache='' free_backup='' free_cache=''
	store_bytes=$(dir_bytes "$STORE_ROOT")
	need_backup=$((2 * TARGET_WEIGHTS_BYTES + 2 * store_bytes))
	need_cache=$((2 * TARGET_WEIGHTS_BYTES))
	free_backup=$(free_bytes "$BACKUP_ROOT")
	free_cache=$(free_bytes "$(cache_root)")
	if [ "$free_backup" -ge "$need_backup" ]; then
		ck_ok "backup+work fs: free=$free_backup B, need ~$need_backup B (2x weights $TARGET_WEIGHTS_BYTES + 2x store $store_bytes)"
	else
		ck_fail "backup+work fs: need ~$need_backup B, have $free_backup B"
	fi
	if [ "$free_cache" -ge "$need_cache" ]; then
		ck_ok "cache fs ($CACHE_ROOT): free=$free_cache B, need ~$need_cache B (2x weights: source read + cache copy)"
	else
		ck_fail "cache fs ($CACHE_ROOT): need ~$need_cache B, have $free_cache B"
	fi

	heading '1f. scopes (read from a COPY of metadata.db)'
	local -a tags=()
	if [ -f "$DATA_DIR/metadata.db" ]; then
		local before=''
		before=$(live_paths_snapshot)
		make_work_dir 1
		# Capture the status: a process substitution discards it, and a failed
		# copy then reads as "active scopes=0" (measured: exit 0, no FAIL).
		if ! enumerate_scopes >"$WORK_RUN/tags"; then
			ck_fail 'cannot enumerate the active scopes from the metadata copy — is the store readable?'
		else
			mapfile -t tags <"$WORK_RUN/tags"
			ck_info "active scopes=${#tags[@]} (the re-embed runs once per tag)"
			if [ "${#tags[@]}" -gt 0 ]; then
				ck_info "first tag: ${tags[0]}"
			fi
			if [ "${#tags[@]}" -eq 0 ]; then
				ck_warn 'metadata.db has no active shard rows — nothing would be re-embedded'
			fi
		fi
		assert_no_new_live_paths "$before" 0
	else
		ck_fail "no $DATA_DIR/metadata.db — cannot enumerate the scopes"
	fi

	heading 'planned actions (nothing here runs during --dry-run)'
	plan "quit OpenCode; confirm with pgrep -af opencode"
	plan "rebuild + re-stage the engine FIRST: rm -rf $(engine_stage_dir)/web-ui/dist && bash $REPO_ROOT/plugin/scripts/stage-engine.sh (the cache root is compiled in; --swap refuses a stale engine)"
	plan "$LIVE_SWAP --backup --store $STORE_ROOT --config-dir $CONFIG_DIR --backup-root $BACKUP_ROOT --yes"
	plan "sh $PROVISIONER --model $TARGET_MODEL --weights $TARGET_WEIGHTS --source $SOURCE_DIR --cache-root $CACHE_ROOT"
	plan "verify weights sha256 = $TARGET_WEIGHTS_SHA256 and $TARGET_WEIGHTS_BYTES B"
	plan "cp -a $CONFIG_DIR/opencode-mem.jsonc <same>.pre-model-swap-<TS>"
	plan "set \"embeddingModel\": \"$TARGET_MODEL\" and \"embeddingDtype\": \"$TARGET_DTYPE\" in place (1 replaced line + 1 inserted line at most)"
	plan "$ENGINE_BIN --mode=reembed-scope --scope <each of ${#tags[@]} tags> --storage-root $DATA_DIR"
	plan 'stop at the first non-zero exit and print the resume command plus the engine rebuild it needs'
	plan "start OpenCode, then $SCRIPT_PATH --verify"
	plan "$SCRIPT_PATH --rollback --yes --from <stamp>   (the rollback path is rehearsed, not improvised)"

	heading 'summary'
	printf '  failures=%s warnings=%s pending=false-start-conditions=%s\n' "$FAILS" "$WARNINGS" "$PENDING"
	if [ "$FAILS" -ne 0 ]; then
		printf '  a FAIL line must be fixed before any mode touches the live host\n'
	fi
}

# ── modes ─────────────────────────────────────────────────────────────────
begin_writing_log() {
	mkdir -p "$BACKUP_ROOT" || die "cannot create the backup root: $BACKUP_ROOT"
	LOG_FILE=$BACKUP_ROOT/adopt.log
	log "adopt-embedding-model.sh mode=$MODE store=$STORE_ROOT backups=$BACKUP_ROOT engine=$ENGINE_BIN"
}

run_backup_mode() {
	require_store
	require_config
	resolve_target
	require_stopped
	begin_writing_log
	STAMP=$(utc_stamp)
	make_backup
	heading 'done'
	printf 'backup path : %s\n' "$BACKUP_ROOT/$BACKUP_TS"
	printf 'next        : %s --swap --yes   (this made no change to the config or the model)\n' "$SCRIPT_PATH"
}

run_provision_mode() {
	resolve_target
	require_engine
	# A writing mode: the provisioner's default invocation canonicalises the
	# store's nomic tokenizer IN PLACE (measured: tokenizer.json 1.0 -> 2.0
	# through a sibling temp file + mv), so it needs the same guards as --swap.
	require_store
	require_stopped
	begin_writing_log
	STAMP=$(utc_stamp)
	provision
	heading 'done'
	printf 'the cache is provisioned; the engine loads it on its next start.\n'
	printf 'after a reboot, re-run: %s --provision\n' "$SCRIPT_PATH"
}

run_swap_mode() {
	require_store
	require_config
	require_engine
	require_engine_reads_store_cache
	resolve_target
	require_stopped
	begin_writing_log
	STAMP=$(utc_stamp)
	make_work_dir 0

	local store_bytes='' before=''
	store_bytes=$(dir_bytes "$STORE_ROOT")
	log "swap: target=$TARGET_MODEL dtype=$TARGET_DTYPE dims=$TARGET_DIMS store=$STORE_ROOT ($store_bytes B)"
	printf '\nthis runs, in order: backup -> provision -> config edit -> re-embed per scope.\n'
	printf 'it does NOT start OpenCode; you do, then you run --verify.\n'
	printf 'every step prints what it will do before it does it.\n'
	# Read the scope list from a COPY of metadata.db before the first write: a
	# copy that cannot be read must abort here, not after the backup and the
	# config flip (measured: it used to read as "no active scopes" and exit 0).
	before=$(live_paths_snapshot)
	if ! enumerate_scopes >"$WORK_RUN/tags"; then
		die 'cannot enumerate the active scopes from the metadata copy — refusing (nothing was written: no backup, no config edit, no re-embed)'
	fi
	assert_no_new_live_paths "$before" 1
	confirm "checkpoint+back up $STORE_ROOT, provision $TARGET_MODEL q8, set embeddingModel/embeddingDtype in $CONFIG_DIR/opencode-mem.jsonc, then re-embed every active scope against the stopped store"
	if [ "$CONFIRMED" -ne 1 ]; then
		log 'the owner declined; nothing was written'
		exit 130
	fi

	make_backup
	provision
	edit_config "$STAMP"
	write_adopt_info
	reembed_all "$WORK_RUN/tags"

	heading 'next'
	printf '\n  STEP 1 — start OpenCode.\n'
	printf '\n  STEP 2 — then run:\n    %s --verify\n' "$SCRIPT_PATH"
	printf 'the engine loads %s from %s and search/add work against the\n' "$TARGET_MODEL" "$(cache_model_dir)"
	printf 're-embedded shards. The config change is the only thing that flips the model.\n'
	printf 'if you have not rebuilt + re-staged the engine since the cache-root change, do\n'
	printf 'that first (the sequence puts it before --swap for exactly this reason).\n'
	final_summary 0
}

# ── --verify ──────────────────────────────────────────────────────────────
HTTP_CODE=
PROBE_BODY=

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
	resolve_target
	command -v jq >/dev/null 2>&1 || die '--verify needs jq to read the JSON envelopes (install jq)'
	begin_writing_log
	heading 'verify — after OpenCode started'

	local -a tags=()
	local verify_before='' enum_ok=1
	make_work_dir 1
	verify_before=$(live_paths_snapshot)
	# Capture the status (a process substitution discards it): a failed copy is
	# not "the store has no active scopes".
	if ! enumerate_scopes >"$WORK_RUN/tags"; then
		enum_ok=0
		ck_fail 'cannot enumerate the active scopes from the metadata copy — is the store readable?'
	fi
	mapfile -t tags <"$WORK_RUN/tags"
	assert_no_new_live_paths "$verify_before" 0
	local first_tag=${tags[0]:-}
	local -a delegate=(--verify --store "$STORE_ROOT" --config-dir "$CONFIG_DIR" --backup-root "$BACKUP_ROOT")
	if [ -n "$REHEARSE_ROOT" ]; then
		delegate+=(--rehearse "$REHEARSE_ROOT")
	fi
	if [ -n "$first_tag" ]; then
		delegate+=(--container-tag "$first_tag")
	fi

	heading 'the live-swap battery (delegated: engine count/parent, web health, add + search, versions)'
	local out='' rc=0
	out=$("$LIVE_SWAP" "${delegate[@]}" 2>&1) || rc=$?
	printf '%s\n' "$out" | sed -n '/^== /,$p' | head -n 120
	if [ "$rc" -ne 0 ]; then
		ck_fail "scripts/live-swap.sh --verify reported FAIL lines (rc $rc) — read its output above"
	fi

	heading 'the model cache (the resolved cache root, not /tmp)'
	ck_info "cache root: $CACHE_ROOT (storagePath=$CONFIG_STORAGE)"
	if is_under "$CACHE_ROOT" "$CONFIG_STORAGE"; then
		ck_ok 'the cache root is under the store (disk-backed, survives a reboot)'
	else
		ck_fail "the cache root $CACHE_ROOT is NOT under $CONFIG_STORAGE — the engine is reading somewhere else"
	fi
	if [ -f "$(cache_model_dir)/$TARGET_WEIGHTS" ]; then
		ck_ok "$TARGET_MODEL q8 is provisioned at $(cache_model_dir)"
		ck_info "weights_bytes=$(wc -c <"$(cache_model_dir)/$TARGET_WEIGHTS" | tr -d ' ')"
	else
		ck_fail "no $TARGET_WEIGHTS under $(cache_model_dir) — run $SCRIPT_PATH --provision"
	fi
	if [ -e "${TMPDIR:-/tmp}/.opencode-mem-onnx" ]; then
		ck_warn "the old tmpfs cache still exists at ${TMPDIR:-/tmp}/.opencode-mem-onnx ($(du -sh "${TMPDIR:-/tmp}/.opencode-mem-onnx" 2>/dev/null | cut -f1) of RAM on a tmpfs) — the engine no longer reads it; reclaim it with: find ${TMPDIR:-/tmp}/.opencode-mem-onnx -depth -delete"
	fi

	heading 'store identity (read from a COPY of one shard)'
	local shard='' copy='' identity='' model='' dims='' fingerprint=''
	refresh_procs
	local pid=''
	pid=$(engine_pids_for_store | head -n 1)
	if [ "$enum_ok" -eq 0 ]; then
		ck_info 'the shard identity read is skipped: the scope enumeration failed (see the FAIL above)'
	elif [ -z "$first_tag" ]; then
		ck_fail 'no active scope in metadata.db — cannot read a shard identity'
	else
		shard=$(scope_shard_db "$first_tag" || true)
		if [ -n "$shard" ] && [ -f "$shard" ]; then
			copy=$(copy_db "$shard" "$WORK_RUN/identity") || die "cannot copy $shard"
			identity=$(shard_identity "$copy")
			model=$(printf '%s\n' "$identity" | sed -n 's/^model=//p')
			dims=$(printf '%s\n' "$identity" | sed -n 's/^dims=//p')
			fingerprint=$(printf '%s\n' "$identity" | sed -n 's/^fingerprint=//p')
			ck_info "scope: $first_tag"
			ck_info "model=${model} dims=${dims}"
			ck_info "fingerprint=${fingerprint}"
			if [ "$model" = "$TARGET_MODEL" ]; then
				ck_ok "the shard's stored model is $TARGET_MODEL"
			else
				ck_fail "the shard's stored model is ${model} (expected $TARGET_MODEL) — the re-embed did not land for this scope"
			fi
			if [ "$dims" = 768 ]; then
				ck_ok 'the shard is 768-wide (no store migration)'
			else
				ck_fail "the shard reports dims=${dims}, expected 768"
			fi
			case $fingerprint in
			*'"model":"'"$TARGET_MODEL"'"'*) ck_ok 'the fingerprint names the target model' ;;
			*) ck_fail "the fingerprint does not name $TARGET_MODEL (got: $fingerprint)" ;;
			esac
			case $fingerprint in
			*'"dtype":"'"$TARGET_DTYPE"'"'*) ck_ok "the fingerprint carries dtype=$TARGET_DTYPE" ;;
			*) ck_warn "the fingerprint does not carry dtype=$TARGET_DTYPE — an explicit dtype may not be reaching the identity" ;;
			esac
		else
			ck_fail "no shard db for scope $first_tag ($shard)"
		fi
	fi

	if [ -n "$pid" ]; then
		heading 'query canaries (Thai and English) and RSS'
		local port='' token='' token_file=$DATA_DIR/.auth-token
		port=$(engine_web_port "$pid")
		ck_info "engine pid $pid, web port $port, parent $(engine_ppid "$pid")"
		if [ -f "$token_file" ]; then
			token=$(tr -d '\n' <"$token_file")
		fi
		local -a canaries=(
			'thai|การปรับแต่งระบบและบันทึกความจำ'
			'english|memory shard layout and search cost'
		)
		local entry='' label='' query='' hits='' top=''
		for entry in "${canaries[@]}"; do
			label=${entry%%|*}
			query=${entry#*|}
			if [ -z "$token" ]; then
				ck_fail "no token at $token_file — cannot run the $label canary"
				continue
			fi
			probe "$port" /api/search --get --data-urlencode "q=$query" -H "x-opencode-mem-token: $token"
			if [ "$HTTP_CODE" = 200 ] && printf '%s' "$PROBE_BODY" | jq -e '.success == true and (.data.items | type) == "array"' >/dev/null 2>&1; then
				hits=$(printf '%s' "$PROBE_BODY" | jq -r '.data.items | length')
				top=$(printf '%s' "$PROBE_BODY" | jq -r '.data.items[0].similarity // "n/a"')
				if [ "$hits" -gt 0 ]; then
					ck_ok "$label canary: $hits hit(s), top similarity $top"
				elif [ "$label" = thai ]; then
					ck_warn "the Thai canary returned 0 hits — search is wired, but either this store has no Thai content or the model is not reading it (the reason for this adoption)"
				else
					ck_warn "the $label canary returned 0 hits — search is wired; the probe query is simply not in this store"
				fi
			else
				ck_fail "$label canary answered HTTP ${HTTP_CODE}: $(printf '%s' "$PROBE_BODY" | head -c 200)"
			fi
		done

		local rss='' hwm=''
		rss=$(sed -n 's/^VmRSS:[[:space:]]*//p' "/proc/$pid/status" 2>/dev/null | tr -d ' ')
		hwm=$(sed -n 's/^VmHWM:[[:space:]]*//p' "/proc/$pid/status" 2>/dev/null | tr -d ' ')
		ck_info "engine RSS: VmRSS=${rss:-unknown} VmHWM=${hwm:-unknown} (fp32 nomic measured 1,152,180 kB on 2026-10-31; q8 e5 should be lower — this is a reading, not a gate)"
	else
		ck_warn 'no engine for this store — start OpenCode, then re-run --verify'
	fi

	heading 'the episodic caveat'
	local epi='' epi_copy='' epi_rows=''
	if [ -f "$DATA_DIR/episodic.db" ]; then
		epi_copy=$(copy_db "$DATA_DIR/episodic.db" "$WORK_RUN/episodic" || true)
		if [ -n "$epi_copy" ]; then
			epi_rows=$(sqlite3 "$epi_copy" 'SELECT count(*) FROM agent_sessions WHERE embedding IS NOT NULL;' 2>/dev/null || true)
		fi
		epi=${epi_rows:-unknown}
		ck_warn "episodic.db has $epi agent_sessions.embedding row(s); they are NEVER re-embedded — pre-adoption rows keep nomic-space vectors beside e5-space ones (same 768 width, so nothing errors; the similarity for those rows is meaningless)"
	else
		ck_info 'no episodic.db in this store'
	fi

	printf '\nMANUAL — what this script cannot check:\n'
	printf '  1. In TWO different workspaces, call the "memory" tool with mode:"list-shards".\n'
	printf '     Each must answer success:true with the same storagePath and shard count.\n'
	printf '  2. A Thai query from the web UI (http://127.0.0.1:%s) should now return\n' "$(engine_web_port "${pid:-0}" 2>/dev/null || printf 3742)"
	printf '     Thai notes that the nomic model could not retrieve (0.19 vs 75.27 nDCG@10).\n'
	printf '  3. If the engine was NOT rebuilt and re-staged after --swap (cd <repo> &&\n'
	printf '     bash plugin/scripts/stage-engine.sh), it still reads the old /tmp cache and\n'
	printf '     would answer -32603: check the "model cache" block above and rebuild.\n'
	printf '  4. After a reboot the cache survives (it is under the store now); only a\n'
	printf '     missing cache needs %s --provision.\n' "$SCRIPT_PATH"
	printf '  5. Keep the backup for a week. Roll back with %s --rollback --yes --from <stamp>.\n' "$SCRIPT_PATH"

	heading 'summary'
	printf '  failures=%s warnings=%s\n' "$FAILS" "$WARNINGS"
	if [ "$FAILS" -ne 0 ]; then
		printf '  fix the FAIL lines above before trusting search.\n'
	fi
}

# print_nomic_cache_note — what a rollback says about the pre-adoption cache.
# It names the BASELINE directory (cache_model_dir is the e5 target) and states
# the condition that is actually true: the file is present and non-canonical,
# not missing.
print_nomic_cache_note() {
	printf 'the nomic cache is at %s; its tokenizer.json was the upstream v1.0 form,\n' "$(baseline_cache_model_dir)"
	printf 'which this engine cannot read — this rollback ran the provisioner to canonicalise\n'
	printf 'it in place (see the [ ok ]/[warn] line above). If that step failed, run:\n'
	printf '  sh %s --model %s --source %s --cache-root %s\n' \
		"$PROVISIONER" "$BASELINE_MODEL" "$(baseline_cache_model_dir)" "$CACHE_ROOT"
}

# ── --rollback ────────────────────────────────────────────────────────────
resolve_adopt_info() {
	local info='' count=''
	if [ -n "$FROM_TS" ]; then
		info=$BACKUP_ROOT/adopt-$FROM_TS.info
		[ -f "$info" ] || die "no adoption record at $info — --from takes the stamp --swap printed (the backup stamp is different)"
	else
		# Never guess a restore point by "newest": a resumed --swap writes a
		# second record whose backup_ts is the MID-migration store while
		# config_copy stays the first run's nomic config (measured: a default
		# rollback restored an e5 shard under a nomic config).
		count=$(find "$BACKUP_ROOT" -maxdepth 1 -type f -name 'adopt-*.info' | wc -l | tr -d ' ')
		if [ "$count" -gt 1 ]; then
			die "$count adoption records under $BACKUP_ROOT — pass --from <stamp> (the one the --swap you mean printed). A resumed run writes a second record whose restore point is the mid-migration store, so \"newest\" is not the record you want."
		fi
		info=$(find "$BACKUP_ROOT" -maxdepth 1 -type f -name 'adopt-*.info' 2>/dev/null | sort | tail -n 1 || true)
		[ -n "$info" ] || die "no adoption record under $BACKUP_ROOT (adopt-<stamp>.info) — nothing to roll back"
	fi
	STAMP=$(sed -n 's/^stamp=//p' "$info" | head -n 1)
	valid_stamp "$STAMP" || die "the adoption record has no valid stamp: $info"
	BACKUP_TS=$(sed -n 's/^backup_ts=//p' "$info" | head -n 1)
	valid_stamp "$BACKUP_TS" || die "the adoption record has no valid backup stamp: $info"
	CONFIG_COPY=$(sed -n 's/^config_copy=//p' "$info" | head -n 1)
	if [ -n "$CONFIG_COPY" ]; then
		case $CONFIG_COPY in
		"$CONFIG_DIR"/*.pre-model-swap-*) ;;
		*) die "the adoption record names a config copy outside $CONFIG_DIR: $CONFIG_COPY" ;;
		esac
		[ -f "$CONFIG_COPY" ] || die "the recorded config copy is gone: $CONFIG_COPY"
	fi
	log "adoption $STAMP: backup=$BACKUP_TS config_copy=${CONFIG_COPY:-<none>}"
}

restore_config() {
	local config=$CONFIG_DIR/opencode-mem.jsonc saved='' target=''
	saved=$BACKUP_ROOT/pre-rollback-$STAMP
	mkdir -p "$saved" || die "cannot create $saved"
	cp -a "$config" "$saved/opencode-mem.jsonc" || die 'cannot save the current config'
	log "current config saved to $saved/opencode-mem.jsonc (kept)"
	target=${CONFIG_COPY:-$BACKUP_ROOT/$BACKUP_TS/config/opencode-mem.jsonc}
	[ -f "$target" ] || die "no pre-swap config to restore (looked at ${CONFIG_COPY:-<none>} and $BACKUP_ROOT/$BACKUP_TS/config/opencode-mem.jsonc)"
	cp -a "$target" "$config" || die "cannot restore $config from $target"
	if ! cmp -s "$target" "$config"; then
		die "the restored config is not byte-identical to $target"
	fi
	ck_ok "config restored byte-identically from $target"
	log "restored $config from $target"
}

# canonicalise_restored_nomic_cache — the restored nomic cache is the upstream
# plugin's artifact: tokenizer.json v1.0, which this engine's reader rejects
# (crates/mem-embed/src/local/tokenize.rs: "accepts canonical tokenizer.json
# v2.0 only"). Measured after a default rollback without this step: rc 74,
# "tokenizer version '1.0' is not `2.0`". The provisioner's default invocation
# is this cache, so it canonicalises the restored file in place (sibling temp
# file + mv). A failure here is a warning, not a rollback failure: the store is
# already restored.
canonicalise_restored_nomic_cache() {
	local dir='' version='' out='' rc=0
	dir=$(baseline_cache_model_dir)
	if [ ! -f "$dir/tokenizer.json" ]; then
		ck_warn "no $BASELINE_MODEL cache in the restored store ($dir) — provision it before starting OpenCode: sh $PROVISIONER"
		return 0
	fi
	version=$(tokenizer_version_of "$dir/tokenizer.json")
	if [ "$version" = 2.0 ]; then
		ck_ok "the restored $BASELINE_MODEL tokenizer is already the canonical v2.0 form ($dir)"
		return 0
	fi
	log "canonicalising the restored $BASELINE_MODEL tokenizer (version ${version:-unknown} -> 2.0) in place"
	out=$(sh "$PROVISIONER" --model "$BASELINE_MODEL" --source "$dir" --cache-root "$CACHE_ROOT" 2>&1) || rc=$?
	if [ "$rc" -ne 0 ]; then
		printf '%s\n' "$out" | tail -n 20 >&2
		ck_warn "the provisioner failed (rc $rc) — $dir/tokenizer.json is still version ${version:-unknown}, which this engine cannot read. Run: sh $PROVISIONER --model $BASELINE_MODEL --source $dir --cache-root $CACHE_ROOT"
		return 0
	fi
	version=$(tokenizer_version_of "$dir/tokenizer.json")
	if [ "$version" = 2.0 ]; then
		ck_ok "the restored $BASELINE_MODEL tokenizer is now the canonical v2.0 form ($dir)"
	else
		ck_warn "the provisioner reported success but $dir/tokenizer.json reads version ${version:-unknown}, not 2.0"
	fi
	return 0
}

run_rollback_mode() {
	require_store
	require_config
	require_stopped
	begin_writing_log
	resolve_adopt_info

	local backup=$BACKUP_ROOT/$BACKUP_TS
	[ -d "$backup/data" ] || die "the backup has no data/: $backup"
	[ -f "$backup/MANIFEST.sha256" ] || die "the backup has no MANIFEST.sha256: $backup — not a restore point"
	if ! (cd "$backup" && sha256sum -c MANIFEST.sha256 --quiet); then
		die "the backup does not verify — not restoring"
	fi
	log "sha256sum -c on $backup: 0 failed"

	heading "rollback to adoption $STAMP (backup $BACKUP_TS)"
	printf 'two routes, both restore the config byte-identically:\n\n'
	printf '  (default) restore the store from the backup:\n'
	printf '      cost   : minutes (one cp -a of %s)\n' "$(du -sh "$backup/data" | cut -f1)"
	printf '      effect : the store is byte-identical to the pre-adoption restore point\n'
	printf '      loss   : every memory written after that backup is gone from the store\n'
	printf '               (the current store is MOVED ASIDE, never deleted: %s/data.model-%s)\n\n' "$BACKUP_ROOT" "$STAMP"
	printf '  (--config-only) restore the config and re-embed with %s:\n' "$BASELINE_MODEL"
	printf "      cost   : a full re-embed of 22 scopes / 3.1k vectors (the brief's\n"
	printf '               40-80 min figure is unverified; the measured q8 e5 run was\n'
	printf '               6.5 s for one 63-memory scope, so treat it as an upper bound)\n'
	printf '      effect : keeps post-backup memories\n'
	printf '      loss   : none, but until each scope finishes the store refuses reads\n'
	printf '               for it with a typed ShardModelMismatch — and episodic.db\n'
	printf '               stays on e5-space vectors either way\n\n'

	if [ "$CONFIG_ONLY" -eq 1 ]; then
		confirm "restore $CONFIG_DIR/opencode-mem.jsonc and print the per-scope re-embed commands (the store is NOT touched by this step)"
		if [ "$CONFIRMED" -ne 1 ]; then
			log 'the owner declined; nothing was written'
			exit 130
		fi
		restore_config
		canonicalise_restored_nomic_cache
		local -a tags=()
		make_work_dir 0
		# Capture the status: a failed copy used to print an empty command list
		# and exit 0 (measured).
		if ! enumerate_scopes >"$WORK_RUN/tags"; then
			die 'cannot enumerate the active scopes from the metadata copy — refusing (the config is restored; no per-scope re-embed command list was produced)'
		fi
		mapfile -t tags <"$WORK_RUN/tags"
		heading 'next (config-only rollback)'
		printf 'the config is back on %s, but every shard still carries %s vectors.\n' "$BASELINE_MODEL" "$TARGET_MODEL"
		printf 'the store refuses reads per scope until that scope is re-embedded. Run, one at a time:\n'
		local tag=''
		for tag in "${tags[@]}"; do
			printf '  %s --mode=reembed-scope --scope %s --storage-root %s\n' "$ENGINE_BIN" "$tag" "$DATA_DIR"
		done
		printf '\n(or take the default route next time: it restores the backed-up store in minutes)\n'
		print_nomic_cache_note
		final_summary 0
		return 0
	fi

	confirm "restore $CONFIG_DIR/opencode-mem.jsonc and move the current store aside, then cp -a $backup/data back (the current store is kept at $BACKUP_ROOT/data.model-$STAMP)"
	if [ "$CONFIRMED" -ne 1 ]; then
		log 'the owner declined; nothing was written'
		exit 130
	fi
	restore_config

	heading 'store restore (move-aside, never delete)'
	local aside=$BACKUP_ROOT/data.model-$STAMP n=2
	while [ -e "$aside" ]; do
		aside=$BACKUP_ROOT/data.model-$STAMP-$n
		n=$((n + 1))
	done
	log "mv $DATA_DIR $aside   (kept for diagnosis)"
	mv "$DATA_DIR" "$aside" || die "cannot move the live store aside: $DATA_DIR"
	if ! cp -a "$backup/data" "$DATA_DIR"; then
		log "FATAL the restore failed; the previous store is at $aside — moving it back"
		find "$DATA_DIR" -depth -delete 2>/dev/null || true
		mv "$aside" "$DATA_DIR" || true
		die 'cp -a of the backup data failed; the previous store was moved back'
	fi
	log "cp -a $backup/data $DATA_DIR"

	heading 'restore verification'
	if diff -r -q "$backup/data" "$DATA_DIR"; then
		ck_ok 'the restored data/ is byte-identical to the backup'
	else
		ck_fail 'the restored data/ differs from the backup — inspect before starting OpenCode'
	fi
	canonicalise_restored_nomic_cache
	heading 'next'
	printf 'start OpenCode; the engine reads %s from the restored store identity.\n' "$BASELINE_MODEL"
	printf 'the pre-adoption store is at: %s   (keep it; delete it yourself once you are sure)\n' "$aside"
	print_nomic_cache_note
	final_summary 0
}

# ── main ──────────────────────────────────────────────────────────────────
main() {
	parse_args "$@"
	resolve_paths

	case $MODE in
	dry-run) dry_run ;;
	backup) run_backup_mode ;;
	provision) run_provision_mode ;;
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

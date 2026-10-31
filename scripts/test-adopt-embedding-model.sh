#!/usr/bin/env bash
# Behavioural self-test for scripts/adopt-embedding-model.sh and for the
# --weights/--variant extension of scripts/provision-onnx-cache.sh.
#
# Every case drives the real scripts as subprocesses against a scratch tree
# under <repo>/.tmp/scratch/adopt-selftest/ (a tiny sqlite store shaped like the
# live one: a `shards` table with 3 active scopes, three shard DBs carrying
# `shard_metadata`, and a config fixture with the live file's commented
# alternatives). A fake engine binary answers `--mode=reembed-scope` from the
# environment and, when started without it, behaves like a process holding the
# store.
#
# The harness never writes the live store, the live config, the live backup
# root or the live ONNX cache: TMPDIR is redirected per invocation, and
# --rehearse pins every path under the scratch root.
#
# Usage: bash scripts/test-adopt-embedding-model.sh
# Exit:  0 when all 20 cases pass, 1 otherwise.
set -uo pipefail

SECONDS=0

repo_root=$(git rev-parse --show-toplevel) || {
	printf 'test-adopt: cannot resolve the repository root\n' >&2
	exit 1
}
adopt=${ADOPT:-$repo_root/scripts/adopt-embedding-model.sh}
provisioner=$repo_root/scripts/provision-onnx-cache.sh
work=$repo_root/.tmp/scratch/adopt-selftest
ok=0
total=20
case_reason=
case_out=
case_rc=0
r=

# --- work-dir guard: never the repository root, never outside .tmp/scratch ---
mkdir -p "$work" || {
	printf 'test-adopt: cannot create %s\n' "$work" >&2
	exit 1
}
work_real=$(cd "$work" && pwd -P)
scratch_real=$(cd "$repo_root/.tmp/scratch" && pwd -P)
case $work_real in
"$scratch_real"/*) ;;
*)
	printf 'test-adopt: refusing to run: %s is not under %s\n' "$work_real" "$scratch_real" >&2
	exit 1
	;;
esac
if [ "$work_real" = "$(cd "$repo_root" && pwd -P)" ]; then
	printf 'test-adopt: refusing to run: the work dir resolves to the repository root\n' >&2
	exit 1
fi

bfail() {
	case_reason=$1
	return 1
}

# ── fixtures ──────────────────────────────────────────────────────────────
SCOPES=(aaaaaaaaaaaaaaaa bbbbbbbbbbbbbbbb cccccccccccccccc)

write_config_fixture() {
	# The live file's shape, with the storage path rendered as an absolute
	# scratch path: a `~/.opencode-mem/data` fixture would make --provision
	# write into the LIVE store's cache.
	cat >"$1" <<JSONC
{
    // ============================================
    // Embedding Model (for similarity search)
    // ============================================
    // Default: Nomic Embed v1 (768 dimensions, 8192 context, multilingual)
    "embeddingModel": "Xenova/nomic-embed-text-v1",
    // Local pooling: "mean" (default) | "cls" | "last_token".
    // "embeddingPooling": "mean",
    // Optional local ONNX dtype override (e.g. "q8", "fp32").
    // "embeddingDtype": "q8",
    // Auto-detected dimensions (no need to set manually)
    // "embeddingDimensions": 768,
    // Other recommended local models:
    // "embeddingModel": "Xenova/jina-embeddings-v2-base-en",  // 768 dims
    "webServerEnabled": true,
    "storagePath": "$r/opencode-mem/data"
}
JSONC
}

# The same file with an ACTIVE embeddingDtype (the replace-in-place branch).
write_config_active_dtype() {
	write_config_fixture "$1"
	sed -i 's|^    // "embeddingDtype": "q8",$|    "embeddingDtype": "fp32",|' "$1"
}

# The same file with an ACTIVE embeddingPooling (the refusal branch).
write_config_active_pooling() {
	write_config_fixture "$1"
	sed -i 's|^    // "embeddingPooling": "mean",$|    "embeddingPooling": "mean",|' "$1"
}

# Byte-exact expected result of the insertion branch.
write_expected_inserted() {
	cat >"$1" <<JSONC
{
    // ============================================
    // Embedding Model (for similarity search)
    // ============================================
    // Default: Nomic Embed v1 (768 dimensions, 8192 context, multilingual)
    "embeddingModel": "Xenova/multilingual-e5-base",
    "embeddingDtype": "q8",
    // Local pooling: "mean" (default) | "cls" | "last_token".
    // "embeddingPooling": "mean",
    // Optional local ONNX dtype override (e.g. "q8", "fp32").
    // "embeddingDtype": "q8",
    // Auto-detected dimensions (no need to set manually)
    // "embeddingDimensions": 768,
    // Other recommended local models:
    // "embeddingModel": "Xenova/jina-embeddings-v2-base-en",  // 768 dims
    "webServerEnabled": true,
    "storagePath": "$r/opencode-mem/data"
}
JSONC
}

# Byte-exact expected result of the replace-in-place branch.
write_expected_replaced() {
	cat >"$1" <<JSONC
{
    // ============================================
    // Embedding Model (for similarity search)
    // ============================================
    // Default: Nomic Embed v1 (768 dimensions, 8192 context, multilingual)
    "embeddingModel": "Xenova/multilingual-e5-base",
    // Local pooling: "mean" (default) | "cls" | "last_token".
    // "embeddingPooling": "mean",
    // Optional local ONNX dtype override (e.g. "q8", "fp32").
    "embeddingDtype": "q8",
    // Auto-detected dimensions (no need to set manually)
    // "embeddingDimensions": 768,
    // Other recommended local models:
    // "embeddingModel": "Xenova/jina-embeddings-v2-base-en",  // 768 dims
    "webServerEnabled": true,
    "storagePath": "$r/opencode-mem/data"
}
JSONC
}

write_plugin_fixture() {
	cat >"$1" <<'JSONC'
{
    // plugins loaded by the host
    "plugins": [
        "opencode-antigravity-auth@latest",
        "opencode-mem@latest"
    ],
    "formatter": false
}
JSONC
}

write_source() {
	local dir=$1
	mkdir -p "$dir/onnx"
	printf '{"architectures":["XLMRobertaModel"],"hidden_size":768,"max_position_embeddings":514}\n' >"$dir/config.json"
	printf '{"version":"2.0","model":{"type":"WordPiece"}}\n' >"$dir/tokenizer.json"
	printf '{"tokenizer_class":"XLMRobertaTokenizer"}\n' >"$dir/tokenizer_config.json"
	printf '{"bos_token":"<s>"}\n' >"$dir/special_tokens_map.json"
	printf 'fake-q8-weights-for-the-selftest\n' >"$dir/onnx/model_quantized.onnx"
}

write_fake_engine() {
	cat >"$1" <<'SH'
#!/bin/bash
mode=''
scope=''
while [ $# -gt 0 ]; do
	case $1 in
	--mode=reembed-scope) mode=reembed ;;
	--scope)
		scope=$2
		shift
		;;
	--storage-root) shift ;;
	esac
	shift
done
if [ "$mode" = reembed ]; then
	printf '%s\n' "$scope" >>"${FAKE_ENGINE_LOG:-/dev/null}"
	if [ -n "${FAKE_ENGINE_FAIL_SCOPE:-}" ] && [ "$scope" = "${FAKE_ENGINE_FAIL_SCOPE}" ]; then
		printf '{"event":"reembed.completed","success":false,"strategy":"re-embed","re_embedded_memories":0,"duration_ms":1}\n'
		printf '{"event":"reembed.failed","level":"error","detail":"simulated failure"}\n' >&2
		exit 74
	fi
	printf '{"event":"reembed.completed","success":true,"strategy":"re-embed","re_embedded_memories":7,"duration_ms":1}\n'
	exit 0
fi
while :; do sleep 1; done
SH
	chmod +x "$1"
}

fake_sha() { sha256sum "$r/source/onnx/model_quantized.onnx" | cut -d' ' -f1; }
fake_bytes() { wc -c <"$r/source/onnx/model_quantized.onnx" | tr -d ' '; }

# mkrehearsal <name> [config-writer] — a scratch root with a 3-scope store,
# the config fixture, a fake model source and the fake engine.
mkrehearsal() {
	local name=$1 writer=${2:-write_config_fixture}
	r=$work/$name
	find "$r" -depth -delete 2>/dev/null
	mkdir -p "$r/opencode-mem/data/projects" "$r/config" "$r/bin" "$r/source" "$r/tmp" || return 1
	sqlite3 "$r/opencode-mem/data/metadata.db" <<'SQL' >/dev/null || return 1
CREATE TABLE shards (id INTEGER PRIMARY KEY AUTOINCREMENT, scope TEXT NOT NULL, scope_hash TEXT NOT NULL, shard_index INTEGER NOT NULL, db_path TEXT NOT NULL, vector_count INTEGER DEFAULT 0, is_active INTEGER DEFAULT 1, created_at INTEGER NOT NULL, UNIQUE (scope, scope_hash, shard_index));
CREATE INDEX idx_active_shards ON shards (scope, scope_hash, is_active);
CREATE UNIQUE INDEX idx_shards_one_active ON shards (scope, scope_hash) WHERE is_active = 1;
INSERT INTO shards (scope, scope_hash, shard_index, db_path, vector_count, is_active, created_at) VALUES
  ('project', 'aaaaaaaaaaaaaaaa', 0, 'projects/project_aaaaaaaaaaaaaaaa_shard_0.db', 10, 1, 1),
  ('project', 'bbbbbbbbbbbbbbbb', 0, 'projects/project_bbbbbbbbbbbbbbbb_shard_0.db', 20, 1, 2),
  ('project', 'cccccccccccccccc', 0, 'projects/project_cccccccccccccccc_shard_0.db', 30, 1, 3);
SQL
	local h=''
	for h in "${SCOPES[@]}"; do
		sqlite3 "$r/opencode-mem/data/projects/project_${h}_shard_0.db" <<SQL >/dev/null || return 1
CREATE TABLE shard_metadata (key TEXT PRIMARY KEY, value TEXT);
INSERT INTO shard_metadata (key, value) VALUES
  ('embedding_model', 'Xenova/nomic-embed-text-v1'),
  ('embedding_dimensions', '768');
CREATE TABLE memories (id TEXT PRIMARY KEY);
INSERT INTO memories VALUES ('m1');
SQL
	done
	printf 'a memory log line\n' >"$r/opencode-mem/opencode-mem.log"
	"$writer" "$r/config/opencode-mem.jsonc"
	cp -a "$r/config/opencode-mem.jsonc" "$r/original.jsonc"
	write_plugin_fixture "$r/config/opencode.jsonc"
	write_source "$r/source"
	write_fake_engine "$r/bin/opencode-agents-memory"
	return 0
}

# ── helpers ───────────────────────────────────────────────────────────────
tree_snapshot() {
	(
		cd "$1" || exit 1
		find . -type f -print0 | sort -z | xargs -0 -r stat -c '%n %s %Y'
	)
}

sha_tree() {
	(
		cd "$1" || exit 1
		find . -type f -print0 | sort -z | xargs -0 -r sha256sum
	)
}

# invoke <args...> — run adopt with TMPDIR pinned into the scratch root.
invoke() {
	case_out=$(TMPDIR="$r/tmp" "$adopt" "$@" 2>&1)
	case_rc=$?
	return 0
}

# swap_args — the standard rehearsal --swap invocation.
swap_args() {
	SWAP_ARGS=(
		--swap --rehearse "$r" --yes
		--source "$r/source"
		--engine "$r/bin/opencode-agents-memory"
		--weights-sha256 "$(fake_sha)"
		--weights-bytes "$(fake_bytes)"
	)
}

# start_fake_engine — a process whose cmdline holds the scratch storage root.
fake_pid=
start_fake_engine() {
	"$r/bin/opencode-agents-memory" --storage-root "$r/opencode-mem/data" >/dev/null 2>&1 &
	fake_pid=$!
	local i=0
	for ((i = 0; i < 50; i++)); do
		[ -r "/proc/$fake_pid/cmdline" ] && return 0
		sleep 0.1
	done
	return 1
}

stop_fake_engine() {
	[ -n "${fake_pid:-}" ] || return 0
	kill "$fake_pid" 2>/dev/null || true
	wait "$fake_pid" 2>/dev/null || true
	fake_pid=
}

# ── cases: the executor ───────────────────────────────────────────────────
case_01_help_names_every_mode() {
	local out=''
	if ! out=$("$adopt" --help 2>&1); then
		bfail '--help exited non-zero'
		return 1
	fi
	case $out in *'--dry-run'*) ;; *) bfail '--help does not name --dry-run'; return 1 ;; esac
	case $out in *'--backup'*) ;; *) bfail '--help does not name --backup'; return 1 ;; esac
	case $out in *'--provision'*) ;; *) bfail '--help does not name --provision'; return 1 ;; esac
	case $out in *'--swap'*) ;; *) bfail '--help does not name --swap'; return 1 ;; esac
	case $out in *'--verify'*) ;; *) bfail '--help does not name --verify'; return 1 ;; esac
	case $out in *'--rollback'*) ;; *) bfail '--help does not name --rollback'; return 1 ;; esac
	case $out in *'docs/runbooks/adopt-embedding-model.md'*) ;; *) bfail '--help does not point at the runbook'; return 1 ;; esac
	return 0
}

case_02_bare_invocation_writes_nothing() {
	mkrehearsal bare || { bfail 'fixture setup failed'; return 1; }
	local before='' after=''
	before=$(tree_snapshot "$r")
	invoke --rehearse "$r" --source "$r/source" --engine "$r/bin/opencode-agents-memory" \
		--weights-sha256 "$(fake_sha)" --weights-bytes "$(fake_bytes)"
	if [ "$case_rc" -ne 0 ]; then
		bfail "a bare invocation exited non-zero: $case_out"
		return 1
	fi
	after=$(tree_snapshot "$r")
	if [ "$before" != "$after" ]; then
		bfail 'a bare invocation wrote inside the rehearsal root'
		return 1
	fi
	case $case_out in *'dry-run'*) ;; *) bfail 'a bare invocation did not run the dry-run mode'; return 1 ;; esac
	case $case_out in *'active scopes=3'*) ;; *) bfail 'the dry-run did not enumerate 3 scopes from the copied metadata.db'; return 1 ;; esac
	return 0
}

case_03_dry_run_reports_and_swap_refuses_a_running_engine() {
	mkrehearsal running || { bfail 'fixture setup failed'; return 1; }
	start_fake_engine || { bfail 'the fake engine did not start'; return 1; }
	invoke --rehearse "$r" --source "$r/source" --engine "$r/bin/opencode-agents-memory" \
		--weights-sha256 "$(fake_sha)" --weights-bytes "$(fake_bytes)"
	if [ "$case_rc" -ne 0 ]; then
		stop_fake_engine
		bfail "the dry-run must stay read-only and exit 0: $case_out"
		return 1
	fi
	case $case_out in *'[pend] engine running for this store'*) ;; *) stop_fake_engine; bfail 'the dry-run did not report the running engine as pending'; return 1 ;; esac
	local before=''
	before=$(sha_tree "$r/opencode-mem")
	swap_args
	invoke "${SWAP_ARGS[@]}"
	stop_fake_engine
	if [ "$case_rc" -ne 1 ]; then
		bfail "a swap while an engine holds the store must be refused (rc 1), got $case_rc"
		return 1
	fi
	case $case_out in *'engine is still running'*) ;; *) bfail 'the refusal does not name the running engine'; return 1 ;; esac
	if [ "$(sha_tree "$r/opencode-mem")" != "$before" ]; then
		bfail 'the refused swap wrote the store'
		return 1
	fi
	if [ -e "$r/backups/LATEST" ]; then
		bfail 'the refused swap made a backup'
		return 1
	fi
	return 0
}

case_04_a_live_pid_in_the_engine_lock_is_refused() {
	mkrehearsal lock || { bfail 'fixture setup failed'; return 1; }
	printf '{"pid":%d,"startedAt":"2026-10-31T00:00:00Z"}\n' "$$" >"$r/opencode-mem/data/.engine.lock"
	swap_args
	invoke "${SWAP_ARGS[@]}"
	if [ "$case_rc" -ne 1 ]; then
		bfail "a store whose .engine.lock names a live pid must be refused (rc 1), got $case_rc"
		return 1
	fi
	case $case_out in *'still owned'*) ;; *) bfail 'the refusal does not say the store is still owned'; return 1 ;; esac
	if [ -e "$r/backups/LATEST" ]; then
		bfail 'the refused swap published a backup'
		return 1
	fi
	return 0
}

case_05_a_384_dimension_model_is_refused() {
	mkrehearsal dim384 || { bfail 'fixture setup failed'; return 1; }
	local before=''
	before=$(sha_tree "$r/config")
	invoke --swap --rehearse "$r" --yes --model Xenova/all-MiniLM-L6-v2 \
		--source "$r/source" --engine "$r/bin/opencode-agents-memory"
	if [ "$case_rc" -ne 1 ]; then
		bfail "a 384-dim target must be refused (rc 1), got $case_rc"
		return 1
	fi
	case $case_out in *'384-dim'*) ;; *) bfail 'the refusal does not name the 384 dimensions'; return 1 ;; esac
	case $case_out in *'768-wide'*) ;; *) bfail 'the refusal does not explain the 768-wide store'; return 1 ;; esac
	if [ "$(sha_tree "$r/config")" != "$before" ]; then
		bfail 'the refused run modified the config'
		return 1
	fi
	if [ -e "$r/backups/LATEST" ]; then
		bfail 'the refused run made a backup before resolving the target'
		return 1
	fi
	return 0
}

case_06_a_1024_dimension_model_is_refused() {
	mkrehearsal dim1024 || { bfail 'fixture setup failed'; return 1; }
	invoke --swap --rehearse "$r" --yes --model Xenova/bge-m3 \
		--source "$r/source" --engine "$r/bin/opencode-agents-memory"
	if [ "$case_rc" -ne 1 ]; then
		bfail "a 1024-dim target must be refused (rc 1), got $case_rc"
		return 1
	fi
	case $case_out in *'1024-dim'*) ;; *) bfail 'the refusal does not name the 1024 dimensions'; return 1 ;; esac
	return 0
}

case_07_swap_inserts_the_dtype_line_byte_exactly() {
	mkrehearsal insert || { bfail 'fixture setup failed'; return 1; }
	local expected=$work/expected-inserted.jsonc
	write_expected_inserted "$expected"
	local original=$r/original.jsonc
	swap_args
	invoke "${SWAP_ARGS[@]}"
	if [ "$case_rc" -ne 0 ]; then
		bfail "the swap failed: $case_out"
		return 1
	fi
	if ! cmp -s "$expected" "$r/config/opencode-mem.jsonc"; then
		bfail 'the edited config is not byte-identical to the expected fixture'
		return 1
	fi
	local pre=''
	pre=$(find "$r/config" -maxdepth 1 -name 'opencode-mem.jsonc.pre-model-swap-*' | sort | tail -n 1)
	if [ -z "$pre" ]; then
		bfail 'no .pre-model-swap-<TS> copy was kept'
		return 1
	fi
	if ! cmp -s "$original" "$pre"; then
		bfail 'the pre-swap copy is not the original config'
		return 1
	fi
	if grep -q '^    "embeddingPooling"' "$r/config/opencode-mem.jsonc"; then
		bfail 'the edit added an embeddingPooling key'
		return 1
	fi
	if grep -q '^    "embeddingDimensions"' "$r/config/opencode-mem.jsonc"; then
		bfail 'the edit added an embeddingDimensions key'
		return 1
	fi
	# per-scope reporting and the re-embed order
	local h=''
	for h in "${SCOPES[@]}"; do
		case $case_out in
		*"[ rc ] opencode_project_${h} rc=0 success=true"*) ;;
		*)
			bfail "no rc line for scope $h"
			return 1
			;;
		esac
	done
	if ! find "$r/backups" -maxdepth 1 -name 'adopt-*.info' | grep -q .; then
		bfail 'no adopt-<stamp>.info record was written'
		return 1
	fi
	if [ ! -f "$r/backups/LATEST" ]; then
		bfail 'the delegated backup did not publish LATEST'
		return 1
	fi
	local ts='' dir=''
	ts=$(cat "$r/backups/LATEST")
	dir=$r/backups/$ts
	if [ ! -f "$dir/MANIFEST.sha256" ] || ! (cd "$dir" && sha256sum -c MANIFEST.sha256 --quiet >/dev/null 2>&1); then
		bfail 'the delegated backup manifest does not verify'
		return 1
	fi
	return 0
}

case_08_swap_replaces_an_active_dtype_line_only() {
	mkrehearsal replace write_config_active_dtype || { bfail 'fixture setup failed'; return 1; }
	local expected=$work/expected-replaced.jsonc
	write_expected_replaced "$expected"
	swap_args
	invoke "${SWAP_ARGS[@]}"
	if [ "$case_rc" -ne 0 ]; then
		bfail "the swap failed: $case_out"
		return 1
	fi
	if ! cmp -s "$expected" "$r/config/opencode-mem.jsonc"; then
		bfail 'the replace-in-place edit is not byte-identical to the expected fixture'
		return 1
	fi
	# exactly one removed line and one added line
	local removed='' added=''
	removed=$( { diff --unchanged-line-format='' --old-line-format='-%L' --new-line-format='' \
		"$r/original.jsonc" "$r/config/opencode-mem.jsonc" || true; } | wc -l | tr -d ' ')
	added=$( { diff --unchanged-line-format='' --old-line-format='' --new-line-format='+%L' \
		"$r/original.jsonc" "$r/config/opencode-mem.jsonc" || true; } | wc -l | tr -d ' ')
	if [ "$removed" -ne 2 ] || [ "$added" -ne 2 ]; then
		bfail "expected 2 changed lines (model + dtype), got $removed removed / $added added"
		return 1
	fi
	return 0
}

case_09_swap_refuses_an_active_pooling_key() {
	mkrehearsal pooling write_config_active_pooling || { bfail 'fixture setup failed'; return 1; }
	local before=''
	before=$(sha_tree "$r/config")
	swap_args
	invoke "${SWAP_ARGS[@]}"
	if [ "$case_rc" -ne 1 ]; then
		bfail "an active embeddingPooling must be refused (rc 1), got $case_rc"
		return 1
	fi
	case $case_out in *'explicit key beats the multilingual-e5 preset'*) ;; *) bfail 'the refusal does not explain the preset precedence'; return 1 ;; esac
	if [ "$(sha_tree "$r/config")" != "$before" ]; then
		bfail 'the refused edit still wrote the config'
		return 1
	fi
	return 0
}

case_10_the_live_root_gains_no_new_path() {
	mkrehearsal nopath || { bfail 'fixture setup failed'; return 1; }
	local before='' after=''
	before=$(find "$r/opencode-mem/data" -mindepth 1 -maxdepth 2 -print | sort)
	swap_args
	invoke "${SWAP_ARGS[@]}"
	if [ "$case_rc" -ne 0 ]; then
		bfail "the swap failed: $case_out"
		return 1
	fi
	after=$(find "$r/opencode-mem/data" -mindepth 1 -maxdepth 2 -print | sort | grep -vF "$r/opencode-mem/data/.cache")
	before=$(printf '%s\n' "$before" | grep -vF "$r/opencode-mem/data/.cache")
	if [ "$before" != "$after" ]; then
		bfail "the store root gained or lost path(s) outside data/.cache: $(comm -3 <(printf '%s\n' "$before") <(printf '%s\n' "$after") | tr '\n' ' ')"
		return 1
	fi
	if [ -n "$(find "$r/opencode-mem/data" -name '*-shm' -print -quit)" ]; then
		bfail 'a live -shm appeared — a live DB was opened'
		return 1
	fi
	# the copies live in the work dir, not in the store
	if ! find "$r/work" -path '*/metadata/metadata.db' | grep -q .; then
		bfail 'no metadata.db copy in the work dir — the enumeration did not use a copy'
		return 1
	fi
	return 0
}

case_11_a_mid_run_failure_stops_and_prints_the_resume() {
	mkrehearsal midfail || { bfail 'fixture setup failed'; return 1; }
	swap_args
	case_out=$(TMPDIR="$r/tmp" FAKE_ENGINE_LOG="$r/engine.log" \
		FAKE_ENGINE_FAIL_SCOPE=opencode_project_bbbbbbbbbbbbbbbb \
		"$adopt" "${SWAP_ARGS[@]}" 2>&1)
	case_rc=$?
	if [ "$case_rc" -ne 74 ]; then
		bfail "a scope answering success:false must exit 74, got $case_rc"
		return 1
	fi
	case $case_out in *'== STOPPED =='*) ;; *) bfail 'no STOPPED block'; return 1 ;; esac
	case $case_out in *'opencode_project_bbbbbbbbbbbbbbbb'*) ;; *) bfail 'the STOPPED block does not name the failing scope'; return 1 ;; esac
	case $case_out in *'not yet attempted (1): opencode_project_cccccccccccccccc'*) ;; *) bfail 'the STOPPED block does not name the remaining scope'; return 1 ;; esac
	case $case_out in *'ShardModelMismatch'*) ;; *) bfail 'the STOPPED block does not state the typed refusal the store will answer'; return 1 ;; esac
	case $case_out in *'resume'*) ;; *) bfail 'no resume command was printed'; return 1 ;; esac
	case $case_out in *'--swap --yes --scope opencode_project_cccccccccccccccc'*) ;; *) bfail 'the resume command does not name the remaining scope'; return 1 ;; esac
	if [ "$(wc -l <"$r/engine.log" | tr -d ' ')" -ne 2 ]; then
		bfail 'the re-embed did not stop at the failing scope (expected 2 engine calls)'
		return 1
	fi
	return 0
}

case_12_scope_filter_limits_the_reembed() {
	mkrehearsal scoped || { bfail 'fixture setup failed'; return 1; }
	swap_args
	SWAP_ARGS+=(--scope opencode_project_cccccccccccccccc)
	case_out=$(TMPDIR="$r/tmp" FAKE_ENGINE_LOG="$r/engine.log" "$adopt" "${SWAP_ARGS[@]}" 2>&1)
	case_rc=$?
	if [ "$case_rc" -ne 0 ]; then
		bfail "the scoped swap failed: $case_out"
		return 1
	fi
	if [ "$(wc -l <"$r/engine.log" | tr -d ' ')" -ne 1 ]; then
		bfail "the --scope filter did not limit the re-embed (got $(wc -l <"$r/engine.log") calls)"
		return 1
	fi
	if [ "$(cat "$r/engine.log")" != opencode_project_cccccccccccccccc ]; then
		bfail 'the wrong scope was re-embedded'
		return 1
	fi
	return 0
}

case_13_rollback_restores_config_and_store_byte_identically() {
	mkrehearsal rollback || { bfail 'fixture setup failed'; return 1; }
	local original=$r/original.jsonc
	local store_before=''
	store_before=$(sha_tree "$r/opencode-mem/data")
	swap_args
	invoke "${SWAP_ARGS[@]}"
	if [ "$case_rc" -ne 0 ]; then
		bfail "the swap failed: $case_out"
		return 1
	fi
	local info='' stamp='' backup_ts='' backup_data=''
	info=$(find "$r/backups" -maxdepth 1 -name 'adopt-*.info' | sort | tail -n 1)
	stamp=$(sed -n 's/^stamp=//p' "$info")
	backup_ts=$(sed -n 's/^backup_ts=//p' "$info")
	backup_data=$(sha_tree "$r/backups/$backup_ts/data")
	# post-adoption drift: a new file and a changed shard
	printf 'written after the adoption\n' >"$r/opencode-mem/data/post-adoption.txt"
	printf 'x' >>"$r/opencode-mem/data/projects/project_aaaaaaaaaaaaaaaa_shard_0.db"
	invoke --rollback --rehearse "$r" --yes --from "$stamp"
	if [ "$case_rc" -ne 0 ]; then
		bfail "the rollback failed: $case_out"
		return 1
	fi
	if ! cmp -s "$original" "$r/config/opencode-mem.jsonc"; then
		bfail 'the rollback did not restore the config byte-identically'
		return 1
	fi
	if [ "$(sha_tree "$r/opencode-mem/data")" != "$backup_data" ]; then
		bfail 'the restored data/ is not byte-identical to the backup'
		return 1
	fi
	if [ "$(sha_tree "$r/opencode-mem/data")" != "$store_before" ]; then
		bfail 'the restored data/ is not the pre-adoption store'
		return 1
	fi
	if [ ! -f "$r/backups/data.model-$stamp/post-adoption.txt" ]; then
		bfail 'the post-adoption store was not moved aside'
		return 1
	fi
	if [ -f "$r/opencode-mem/data/post-adoption.txt" ]; then
		bfail 'the post-adoption file survived into the restored store'
		return 1
	fi
	return 0
}

case_14_config_only_rollback_leaves_the_store_alone() {
	mkrehearsal cfgonly || { bfail 'fixture setup failed'; return 1; }
	local original=$r/original.jsonc
	swap_args
	invoke "${SWAP_ARGS[@]}"
	if [ "$case_rc" -ne 0 ]; then
		bfail "the swap failed: $case_out"
		return 1
	fi
	local info='' stamp='' store_after=''
	info=$(find "$r/backups" -maxdepth 1 -name 'adopt-*.info' | sort | tail -n 1)
	stamp=$(sed -n 's/^stamp=//p' "$info")
	store_after=$(sha_tree "$r/opencode-mem")
	invoke --rollback --rehearse "$r" --yes --from "$stamp" --config-only
	if [ "$case_rc" -ne 0 ]; then
		bfail "the config-only rollback failed: $case_out"
		return 1
	fi
	if ! cmp -s "$original" "$r/config/opencode-mem.jsonc"; then
		bfail 'the config-only rollback did not restore the config byte-identically'
		return 1
	fi
	if [ "$(sha_tree "$r/opencode-mem")" != "$store_after" ]; then
		bfail 'the config-only rollback touched the store'
		return 1
	fi
	case $case_out in *'--mode=reembed-scope --scope opencode_project_aaaaaaaaaaaaaaaa'*) ;; *) bfail 'the config-only rollback did not print the per-scope re-embed commands'; return 1 ;; esac
	case $case_out in *'a full re-embed of 22 scopes'*) ;; *) bfail 'the rollback did not state the config-only route cost'; return 1 ;; esac
	return 0
}

case_15_provision_refuses_a_sha_mismatch() {
	mkrehearsal prov || { bfail 'fixture setup failed'; return 1; }
	invoke --provision --rehearse "$r" --yes --source "$r/source" \
		--engine "$r/bin/opencode-agents-memory" \
		--weights-sha256 0000000000000000000000000000000000000000000000000000000000000000 \
		--weights-bytes "$(fake_bytes)"
	if [ "$case_rc" -ne 1 ]; then
		bfail "a sha256 mismatch must be refused (rc 1), got $case_rc"
		return 1
	fi
	case $case_out in *'not the pinned'*) ;; *) bfail 'the refusal does not name the pinned digest'; return 1 ;; esac
	return 0
}

case_16_provision_reports_the_pinned_values() {
	mkrehearsal provok || { bfail 'fixture setup failed'; return 1; }
	invoke --provision --rehearse "$r" --yes --source "$r/source" \
		--engine "$r/bin/opencode-agents-memory" \
		--weights-sha256 "$(fake_sha)" --weights-bytes "$(fake_bytes)"
	if [ "$case_rc" -ne 0 ]; then
		bfail "the provision failed: $case_out"
		return 1
	fi
	case $case_out in *"weights_sha256=$(fake_sha)"*) ;; *) bfail 'the report does not carry the verified sha256'; return 1 ;; esac
	case $case_out in *"weights_bytes=$(fake_bytes)"*) ;; *) bfail 'the report does not carry the verified byte count'; return 1 ;; esac
	case $case_out in *'tokenizer_version=2.0'*) ;; *) bfail 'the report does not carry the canonical tokenizer version'; return 1 ;; esac
	if [ ! -f "$r/opencode-mem/data/.cache/Xenova/multilingual-e5-base/onnx/model_quantized.onnx" ]; then
		bfail 'the weights are not in the store cache root'
		return 1
	fi
	if [ -e "$r/tmp/.opencode-mem-onnx" ]; then
		bfail 'the provision wrote into TMPDIR — the cache must be under the store'
		return 1
	fi
	# idempotent: a second run succeeds and reports the same digest
	invoke --provision --rehearse "$r" --yes --source "$r/source" \
		--engine "$r/bin/opencode-agents-memory" \
		--weights-sha256 "$(fake_sha)" --weights-bytes "$(fake_bytes)"
	if [ "$case_rc" -ne 0 ]; then
		bfail "the second provision failed: $case_out"
		return 1
	fi
	return 0
}

case_17_a_malformed_from_stamp_is_rejected() {
	mkrehearsal fromstamp || { bfail 'fixture setup failed'; return 1; }
	invoke --rollback --rehearse "$r" --yes --from ../etc
	if [ "$case_rc" -ne 2 ]; then
		bfail "--from ../etc must exit 2, got $case_rc"
		return 1
	fi
	invoke --rollback --rehearse "$r" --yes --from 2026-10-31
	if [ "$case_rc" -ne 2 ]; then
		bfail "--from 2026-10-31 must exit 2, got $case_rc"
		return 1
	fi
	return 0
}

case_18_rollback_refuses_a_running_engine() {
	mkrehearsal rbrun || { bfail 'fixture setup failed'; return 1; }
	swap_args
	invoke "${SWAP_ARGS[@]}"
	if [ "$case_rc" -ne 0 ]; then
		bfail "the swap failed: $case_out"
		return 1
	fi
	local info='' stamp='' before=''
	info=$(find "$r/backups" -maxdepth 1 -name 'adopt-*.info' | sort | tail -n 1)
	stamp=$(sed -n 's/^stamp=//p' "$info")
	before=$(sha_tree "$r/opencode-mem")
	start_fake_engine || { bfail 'the fake engine did not start'; return 1; }
	invoke --rollback --rehearse "$r" --yes --from "$stamp"
	stop_fake_engine
	if [ "$case_rc" -ne 1 ]; then
		bfail "a rollback while an engine holds the store must be refused (rc 1), got $case_rc"
		return 1
	fi
	if [ "$(sha_tree "$r/opencode-mem")" != "$before" ]; then
		bfail 'the refused rollback modified the store'
		return 1
	fi
	return 0
}

# ── cases: the provisioner extension ──────────────────────────────────────
case_19_provisioner_variant_and_external_data() {
	local d=$work/prov-extension
	find "$d" -depth -delete 2>/dev/null
	mkdir -p "$d/q8/onnx" "$d/stub/onnx"
	printf '{"version":"2.0"}\n' >"$d/q8/tokenizer.json"
	printf 'q8-bytes\n' >"$d/q8/onnx/model_quantized.onnx"
	printf '{"version":"2.0"}\n' >"$d/stub/tokenizer.json"
	printf 'stub\n' >"$d/stub/onnx/model.onnx"
	printf 'huge\n' >"$d/stub/onnx/model.onnx_data"
	local out='' rc=0
	out=$(sh "$provisioner" --source "$d/q8" --cache-root "$d/cache" --variant q8 2>&1) || rc=$?
	if [ "$rc" -ne 0 ]; then
		bfail "--variant q8 failed: $out"
		return 1
	fi
	case $out in *"model_onnx_bytes=$(wc -c <"$d/q8/onnx/model_quantized.onnx" | tr -d ' ')"*) ;; *) bfail '--variant q8 did not report the quantized file'; return 1 ;; esac
	out=$(sh "$provisioner" --source "$d/q8" --cache-root "$d/cache" --variant q5 2>&1)
	rc=$?
	if [ "$rc" -ne 2 ]; then
		bfail "an unknown --variant must exit 2, got $rc"
		return 1
	fi
	out=$(sh "$provisioner" --source "$d/stub" --cache-root "$d/cache2" 2>&1)
	rc=$?
	if [ "$rc" -ne 1 ]; then
		bfail "a stub with a sibling _data must exit 1, got $rc"
		return 1
	fi
	case $out in *'external-data ONNX'*) ;; *) bfail 'the refusal does not name external-data ONNX'; return 1 ;; esac
	if [ -e "$d/cache2" ]; then
		bfail 'the refused provision still created the destination'
		return 1
	fi
	return 0
}

case_20_provisioner_default_output_shape_is_unchanged() {
	local d=$work/prov-default
	find "$d" -depth -delete 2>/dev/null
	mkdir -p "$d/src/onnx"
	printf '{"version":"2.0"}\n' >"$d/src/tokenizer.json"
	printf 'fp32-weights\n' >"$d/src/onnx/model.onnx"
	local out='' rc=0
	out=$(sh "$provisioner" --source "$d/src" --cache-root "$d/cache" 2>&1) || rc=$?
	if [ "$rc" -ne 0 ]; then
		bfail "the default invocation failed: $out"
		return 1
	fi
	local expected_keys='cache_root= model= model_dir= tokenizer_version=2.0 model_onnx_bytes= model_onnx_sha256='
	local line=''
	while IFS= read -r line; do
		case $line in
		cache_root=* | model=* | model_dir=* | tokenizer_version=* | model_onnx_bytes=* | model_onnx_sha256=*) ;;
		*)
			bfail "the default invocation printed an unexpected line: $line"
			return 1
			;;
		esac
	done <<<"$out"
	if [ "$(printf '%s\n' "$out" | wc -l | tr -d ' ')" -ne 6 ]; then
		bfail "the default invocation printed $(printf '%s\n' "$out" | wc -l) lines, expected 6"
		return 1
	fi
	case $out in *"model=Xenova/nomic-embed-text-v1"*) ;; *) bfail 'the default model id changed'; return 1 ;; esac
	case $out in *"model_onnx_bytes=$(wc -c <"$d/src/onnx/model.onnx" | tr -d ' ')"*) ;; *) bfail 'the default bytes do not come from onnx/model.onnx'; return 1 ;; esac
	_=$expected_keys
	return 0
}

# ── runner ────────────────────────────────────────────────────────────────
run() {
	local fn=$1
	case_reason=
	case_out=
	if "$fn"; then
		ok=$((ok + 1))
		printf 'ok   %s\n' "$fn"
	else
		printf 'FAIL %s — %s\n' "$fn" "${case_reason:-unknown failure}"
		if [ -n "$case_out" ]; then
			printf '%s\n' "$case_out" | sed -n '1,25p' | sed 's/^/       /'
		fi
	fi
}

run case_01_help_names_every_mode
run case_02_bare_invocation_writes_nothing
run case_03_dry_run_reports_and_swap_refuses_a_running_engine
run case_04_a_live_pid_in_the_engine_lock_is_refused
run case_05_a_384_dimension_model_is_refused
run case_06_a_1024_dimension_model_is_refused
run case_07_swap_inserts_the_dtype_line_byte_exactly
run case_08_swap_replaces_an_active_dtype_line_only
run case_09_swap_refuses_an_active_pooling_key
run case_10_the_live_root_gains_no_new_path
run case_11_a_mid_run_failure_stops_and_prints_the_resume
run case_12_scope_filter_limits_the_reembed
run case_13_rollback_restores_config_and_store_byte_identically
run case_14_config_only_rollback_leaves_the_store_alone
run case_15_provision_refuses_a_sha_mismatch
run case_16_provision_reports_the_pinned_values
run case_17_a_malformed_from_stamp_is_rejected
run case_18_rollback_refuses_a_running_engine
run case_19_provisioner_variant_and_external_data
run case_20_provisioner_default_output_shape_is_unchanged

printf 'wall time: %ss\n' "$SECONDS"
if [ "$ok" -eq "$total" ]; then
	printf 'PASS %d/%d\n' "$ok" "$total"
else
	printf 'FAIL %d/%d\n' "$ok" "$total"
	exit 1
fi

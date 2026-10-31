#!/usr/bin/env bash
# Behavioural self-test for scripts/live-swap.sh.
#
# Every case drives the real script as a subprocess against a scratch tree under
# <repo>/.tmp/scratch/live-swap-selftest/ (a tiny sqlite store + a config
# fixture shaped like the live opencode.jsonc: a commented plugin line, a URL
# with `//`, trailing keys). The harness never writes the live store, the live
# config or the live backup root; its only git call against the real repo is
# read-only (`rev-parse`).
#
# Usage: bash scripts/test-live-swap.sh
# Exit:  0 when all 18 cases pass, 1 otherwise.
set -uo pipefail

SECONDS=0

repo_root=$(git rev-parse --show-toplevel) || {
	printf 'test-live-swap: cannot resolve the repository root\n' >&2
	exit 1
}
swap=$repo_root/scripts/live-swap.sh
plugin=$repo_root/plugin
work=$repo_root/.tmp/scratch/live-swap-selftest
ok=0
total=18
case_reason=
case_out=

# --- work-dir guard: never the repository root, never outside .tmp/scratch ---
mkdir -p "$work" || {
	printf 'test-live-swap: cannot create %s\n' "$work" >&2
	exit 1
}
work_real=$(cd "$work" && pwd -P)
scratch_real=$(cd "$repo_root/.tmp/scratch" && pwd -P)
case $work_real in
"$scratch_real"/*) ;;
*)
	printf 'test-live-swap: refusing to run: %s is not under %s\n' "$work_real" "$scratch_real" >&2
	exit 1
	;;
esac
if [ "$work_real" = "$(cd "$repo_root" && pwd -P)" ]; then
	printf 'test-live-swap: refusing to run: the work dir resolves to the repository root\n' >&2
	exit 1
fi

bfail() {
	case_reason=$1
	return 1
}

# mkrehearsal <name> — a scratch root with a tiny store and the config fixture.
# Sets $r (the root). Prints nothing.
mkrehearsal() {
	local name=$1
	r=$work/$name
	find "$r" -depth -delete 2>/dev/null
	mkdir -p "$r/opencode-mem/data" "$r/config"
	sqlite3 "$r/opencode-mem/data/metadata.db" 'CREATE TABLE shards (tag TEXT);' >/dev/null || return 1
	sqlite3 "$r/opencode-mem/data/skills.db" 'CREATE TABLE skills (id TEXT);' >/dev/null || return 1
	printf 'a memory log line\n' >"$r/opencode-mem/opencode-mem.log"
	printf '{"storagePath":"~/.opencode-mem/data"}\n' >"$r/config/opencode-mem.jsonc"
	write_config_fixture "$r/config/opencode.jsonc"
}

# write_config_fixture <path> — the live config's shape, non-normalized on
# purpose: a commented-out local plugin line, a URL holding `//`, and a
# trailing scalar key so a naive JSON rewrite is caught.
write_config_fixture() {
	cat >"$1" <<JSONC
{
    // plugins loaded by the host
    "plugins": [
        "@mohak34/opencode-notifier@latest",
        "opencode-antigravity-auth@latest",
        // "$plugin"
        "opencode-mem@latest"
    ],
    "providers": {
        "litellm": {
            "baseURL": "http://10.90.11.16:4000/v1"
        }
    },
    "formatter": false
}
JSONC
}

# write_expected_swapped <path> — the byte-exact expected result of the swap.
write_expected_swapped() {
	cat >"$1" <<JSONC
{
    // plugins loaded by the host
    "plugins": [
        "@mohak34/opencode-notifier@latest",
        "opencode-antigravity-auth@latest",
        // "$plugin"
        "$plugin"
    ],
    "providers": {
        "litellm": {
            "baseURL": "http://10.90.11.16:4000/v1"
        }
    },
    "formatter": false
}
JSONC
}

# tree_snapshot <dir> — a stable "contents + mtimes" fingerprint.
tree_snapshot() {
	(
		cd "$1" || exit 1
		find . -type f -print0 | sort -z | xargs -0 -r stat -c '%n %s %Y'
	)
}

# sha_tree <dir> — sha256 of every file, path-relative, for a byte compare.
sha_tree() {
	(
		cd "$1" || exit 1
		find . -type f -print0 | sort -z | xargs -0 -r sha256sum
	)
}

# invoke <args...> — run the script, capture output and the exact exit code.
case_rc=0
invoke() {
	case_out=$("$swap" "$@" 2>&1)
	case_rc=$?
	return 0
}

# ── cases ─────────────────────────────────────────────────────────────────
case_01_help_names_every_mode() {
	local out=''
	if ! out=$("$swap" --help 2>&1); then
		bfail '--help exited non-zero'
		return 1
	fi
	case $out in *'--dry-run'*) ;; *) bfail '--help does not name --dry-run'; return 1 ;; esac
	case $out in *'--backup'*) ;; *) bfail '--help does not name --backup'; return 1 ;; esac
	case $out in *'--swap'*) ;; *) bfail '--help does not name --swap'; return 1 ;; esac
	case $out in *'--verify'*) ;; *) bfail '--help does not name --verify'; return 1 ;; esac
	case $out in *'--rollback'*) ;; *) bfail '--help does not name --rollback'; return 1 ;; esac
	case $out in *'docs/runbooks/live-swap.md'*) ;; *) bfail '--help does not point at the runbook'; return 1 ;; esac
	return 0
}

case_02_bare_invocation_writes_nothing() {
	mkrehearsal bare || { bfail 'fixture setup failed'; return 1; }
	local before='' after=''
	before=$(tree_snapshot "$r")
	invoke --rehearse "$r"
	if [ "$case_rc" -ne 0 ]; then
		bfail "a bare invocation exited non-zero: $case_out"
		return 1

	fi
	after=$(tree_snapshot "$r")
	if [ "$before" != "$after" ]; then
		bfail 'a bare invocation wrote inside the store root'
		return 1

	fi
	case $case_out in *'dry-run'*) ;; *) bfail 'a bare invocation did not run the dry-run mode'; return 1 ;; esac
	return 0
}

case_03_dry_run_fails_on_a_missing_store() {
	invoke --dry-run --store "$work/does-not-exist"
	if [ "$case_rc" -ne 3 ]; then
		bfail "a missing store must exit 3, got $case_rc"
		return 1

	fi
	case $case_out in *'[FAIL] store root/data not found'*) ;; *) bfail 'the missing store was not reported as FAIL'; return 1 ;; esac
	return 0
}

case_04_swap_replaces_one_active_line_only() {
	mkrehearsal swap || { bfail 'fixture setup failed'; return 1; }
	local expected=$work/expected-swapped.jsonc
	write_expected_swapped "$expected"
	invoke --swap --rehearse "$r" --yes
	if [ "$case_rc" -ne 0 ]; then
		bfail "the swap failed: $case_out"
		return 1

	fi
	if ! cmp -s "$expected" "$r/config/opencode.jsonc"; then
		bfail 'the swapped config is not byte-identical to the expected fixture'
		return 1

	fi
	if ! grep -qF "// \"$plugin\"" "$r/config/opencode.jsonc"; then
		bfail 'the commented plugin line was rewritten'
		return 1

	fi
	if [ "$(grep -cF '"opencode-mem@latest"' "$r/config/opencode.jsonc")" -ne 0 ]; then
		bfail 'the upstream token survived the swap'
		return 1

	fi
	if ! find "$r/config" -maxdepth 1 -name 'opencode.jsonc.pre-swap-*' | grep -q .; then
		bfail 'no .pre-swap-<TS> copy was kept'
		return 1

	fi
	return 0
}

case_05_swap_refuses_a_double_edit() {
	mkrehearsal double || { bfail 'fixture setup failed'; return 1; }
	# both an active upstream entry and an active local entry
	awk -v p="\"$plugin\"" '{ print; if ($0 ~ /"opencode-mem@latest"/) print "        " p "," }' \
		"$r/config/opencode.jsonc" >"$r/config/opencode.jsonc.tmp"
	mv "$r/config/opencode.jsonc.tmp" "$r/config/opencode.jsonc"
	local before=''
	before=$(sha256sum "$r/config/opencode.jsonc" | cut -d' ' -f1)
	invoke --swap --rehearse "$r" --yes
	if [ "$case_rc" -ne 1 ]; then
		bfail "a double edit must be refused (rc 1), got $case_rc"
		return 1

	fi
	if [ "$(sha256sum "$r/config/opencode.jsonc" | cut -d' ' -f1)" != "$before" ]; then
		bfail 'the refused double edit still wrote the config'
		return 1

	fi
	case $case_out in *'double edit'*) ;; *) bfail 'the refusal does not name the double edit'; return 1 ;; esac
	return 0
}

case_06_swap_is_idempotent() {
	mkrehearsal idem || { bfail 'fixture setup failed'; return 1; }
	"$swap" --swap --rehearse "$r" --yes >/dev/null 2>&1 || { bfail 'the first swap failed'; return 1; }
	local first=''
	first=$(sha256sum "$r/config/opencode.jsonc" | cut -d' ' -f1)
	local copies_before=''
	copies_before=$(find "$r/config" -maxdepth 1 -name 'opencode.jsonc.pre-swap-*' | wc -l)
	invoke --swap --rehearse "$r" --yes
	if [ "$case_rc" -ne 0 ]; then
		bfail "the second swap failed: $case_out"
		return 1

	fi
	case $case_out in *'nothing to change (idempotent)'*) ;; *) bfail 'the second swap did not report idempotence'; return 1 ;; esac
	if [ "$(sha256sum "$r/config/opencode.jsonc" | cut -d' ' -f1)" != "$first" ]; then
		bfail 'the second swap modified the config again'
		return 1

	fi
	if [ "$(find "$r/config" -maxdepth 1 -name 'opencode.jsonc.pre-swap-*' | wc -l)" != "$copies_before" ]; then
		bfail 'the second swap made another pre-swap copy'
		return 1

	fi
	return 0
}

case_07_swap_refuses_an_unrelated_config() {
	mkrehearsal unrelated || { bfail 'fixture setup failed'; return 1; }
	printf '{ "plugins": [ "some-other-plugin@latest" ] }\n' >"$r/config/opencode.jsonc"
	local before=''
	before=$(sha256sum "$r/config/opencode.jsonc" | cut -d' ' -f1)
	invoke --swap --rehearse "$r" --yes
	if [ "$case_rc" -ne 1 ]; then
		bfail 'a config with neither token must be refused'
		return 1

	fi
	if [ "$(sha256sum "$r/config/opencode.jsonc" | cut -d' ' -f1)" != "$before" ]; then
		bfail 'the refused config was modified'
		return 1

	fi
	return 0
}

case_08_rehearse_refuses_a_live_path() {
	local live=$HOME/.opencode-mem
	invoke --dry-run --rehearse "$live"
	if [ "$case_rc" -ne 1 ]; then
		bfail '--rehearse pointed at the live store must be refused'
		return 1

	fi
	case $case_out in *'inside the live store'*) ;; *) bfail 'the live-path refusal is not explicit'; return 1 ;; esac
	invoke --dry-run --rehearse "$HOME"
	if [ "$case_rc" -ne 1 ]; then
		bfail '--rehearse pointed at a directory containing the live store must be refused'
		return 1

	fi
	return 0
}

case_09_backup_writes_a_verified_manifest() {
	mkrehearsal backup || { bfail 'fixture setup failed'; return 1; }
	invoke --backup --rehearse "$r" --yes
	if [ "$case_rc" -ne 0 ]; then
		bfail "the backup failed: $case_out"
		return 1

	fi
	local latest='' dir=''
	latest=$(cat "$r/backups/LATEST" 2>/dev/null)
	if [ -z "$latest" ]; then
		bfail 'no LATEST pointer'
		return 1

	fi
	dir=$r/backups/$latest
	if [ ! -f "$dir/MANIFEST.sha256" ]; then
		bfail 'no MANIFEST.sha256'
		return 1
	fi
	if [ ! -f "$dir/BACKUP-INFO" ]; then
		bfail 'no BACKUP-INFO completeness marker'
		return 1
	fi
	if ! (cd "$dir" && sha256sum -c MANIFEST.sha256 --quiet); then
		bfail 'the manifest does not verify'
		return 1

	fi
	if ! grep -qF './data/metadata.db' "$dir/MANIFEST.sha256"; then
		bfail 'the manifest omits the database'
		return 1
	fi
	if [ ! -f "$dir/data/metadata.db" ]; then
		bfail 'the database is not in the backup'
		return 1
	fi
	if [ ! -f "$dir/config/opencode.jsonc" ]; then
		bfail 'the config is not in the backup'
		return 1
	fi
	# no derived sidecars left behind by the checkpoint
	if [ -n "$(find "$dir/data" -type f \( -name '*-shm' -o -name '*-tshm' \) -print -quit)" ]; then
		bfail 'the backup carries derived *-shm/*-tshm sidecars'
		return 1

	fi
	return 0
}

case_10_the_manifest_is_behavioural() {
	mkrehearsal manifest || { bfail 'fixture setup failed'; return 1; }
	"$swap" --backup --rehearse "$r" --yes >/dev/null 2>&1 || { bfail 'the backup failed'; return 1; }
	local latest='' dir=''
	latest=$(cat "$r/backups/LATEST")
	dir=$r/backups/$latest
	# one byte appended to a copied database must break the verification
	printf 'x' >>"$dir/data/metadata.db"
	if (cd "$dir" && sha256sum -c MANIFEST.sha256 --quiet >/dev/null 2>&1); then
		bfail 'a mutated copy still verifies — the manifest gate is prose'
		return 1

	fi
	return 0
}

case_11_rollback_refuses_a_post_swap_backup() {
	mkrehearsal postswap || { bfail 'fixture setup failed'; return 1; }
	local pre=$r/backups/20260101T000000Z post=$r/backups/20260102T000000Z
	mkdir -p "$pre/data" "$pre/config" "$post/data" "$post/config"
	printf 'x\n' >"$pre/BACKUP-INFO"
	printf '%s\n' "$(cd "$pre" && sha256sum /dev/null)" >"$pre/MANIFEST.sha256"
	printf 'x\n' >"$post/BACKUP-INFO"
	printf 'x\n' >"$post/MANIFEST.sha256"
	write_config_fixture "$pre/config/opencode.jsonc"
	cp -a "$r/config/opencode-mem.jsonc" "$pre/config/opencode-mem.jsonc"
	write_expected_swapped "$post/config/opencode.jsonc"
	cp -a "$r/config/opencode-mem.jsonc" "$post/config/opencode-mem.jsonc"
	cp -a "$r/opencode-mem/data/metadata.db" "$pre/data/metadata.db"
	cp -a "$r/opencode-mem/data/metadata.db" "$post/data/metadata.db"
	local before=''
	before=$(sha_tree "$r/opencode-mem")
	invoke --rollback --rehearse "$r" --yes --from 20260102T000000Z
	if [ "$case_rc" -ne 1 ]; then
		bfail 'a post-swap backup must be refused as a restore point'
		return 1

	fi
	case $case_out in *'was taken AFTER the swap'*) ;; *) bfail 'the refusal does not explain the post-swap config'; return 1 ;; esac
	case $case_out in *'20260101T000000Z'*) ;; *) bfail 'the refusal does not name the pre-swap backup'; return 1 ;; esac
	if [ "$(sha_tree "$r/opencode-mem")" != "$before" ]; then
		bfail 'the refused rollback modified the store'
		return 1

	fi
	return 0
}

case_12_rollback_restores_bytes_and_keeps_the_port_era_store() {
	mkrehearsal restore || { bfail 'fixture setup failed'; return 1; }
	"$swap" --backup --rehearse "$r" --yes >/dev/null 2>&1 || { bfail 'the backup failed'; return 1; }
	local ts='' dir=''
	ts=$(cat "$r/backups/LATEST")
	dir=$r/backups/$ts
	local expected=''
	expected=$(sha_tree "$dir/data")
	# the port era: a migrated version and an extra row-level file
	sqlite3 "$r/opencode-mem/data/metadata.db" 'PRAGMA user_version=2;' >/dev/null
	printf 'port era\n' >"$r/opencode-mem/data/port-era.txt"
	invoke --rollback --rehearse "$r" --yes --from "$ts"
	if [ "$case_rc" -ne 0 ]; then
		bfail "the rollback failed: $case_out"
		return 1

	fi
	if [ "$(sha_tree "$r/opencode-mem/data")" != "$expected" ]; then
		bfail 'the restored data/ is not byte-identical to the backup'
		return 1

	fi
	if [ ! -f "$r/backups/data.port-$ts/port-era.txt" ]; then
		bfail 'the port-era store was not moved aside'
		return 1
	fi
	if [ -f "$r/opencode-mem/data/port-era.txt" ]; then
		bfail 'the port-era file survived into the restored store'
		return 1
	fi
	# §4.1(a)/§4.7: with no LATEST a bare --rollback falls back to the newest
	# backup carrying BOTH MANIFEST.sha256 and BACKUP-INFO. The fallback note
	# must not be captured into the stamp (it was, before RESOLVED_TS).
	mv "$r/backups/LATEST" "$r/backups/LATEST.hold"
	invoke --rollback --rehearse "$r" --yes
	if [ "$case_rc" -ne 0 ]; then
		bfail "the no-LATEST fallback failed: $case_out"
		return 1
	fi
	case $case_out in
	*"no LATEST pointer; using the newest dated backup: $ts"*) ;;
	*)
		bfail 'the no-LATEST fallback did not name the backup it used'
		return 1
		;;
	esac
	if [ "$(sha_tree "$r/opencode-mem/data")" != "$expected" ]; then
		bfail 'the fallback restore is not byte-identical to the backup'
		return 1
	fi
	return 0
}

case_13_backup_does_not_publish_a_bad_restore_point() {
	mkrehearsal corrupt || { bfail 'fixture setup failed'; return 1; }
	printf 'this is not a database\n' >"$r/opencode-mem/data/broken.db"
	invoke --backup --rehearse "$r" --yes
	if [ "$case_rc" -eq 0 ]; then
		bfail 'a corrupt database did not fail the backup'
		return 1

	fi
	if [ -f "$r/backups/LATEST" ]; then
		bfail 'LATEST was published for an unverified backup'
		return 1

	fi
	case $case_out in *'integrity_check failed'*) ;; *) bfail 'the integrity failure is not named'; return 1 ;; esac
	return 0
}

case_14_backup_root_must_live_outside_the_store() {
	mkrehearsal outroot || { bfail 'fixture setup failed'; return 1; }
	# No --rehearse on purpose: --rehearse pins the backup root under the scratch
	# root, so the outside-the-store guard is only reachable through --store /
	# --config-dir. resolve_paths refuses before any process check or write.
	invoke --backup --store "$r/opencode-mem" --config-dir "$r/config" \
		--backup-root "$r/opencode-mem/backups" --yes
	if [ "$case_rc" -ne 1 ]; then
		bfail "a backup root inside the store must be refused (rc 1), got $case_rc"
		return 1
	fi
	if ! printf '%s' "$case_out" | grep -qF 'must live OUTSIDE'; then
		bfail 'the refusal does not name the outside-the-store rule'
		return 1
	fi
	if [ -e "$r/opencode-mem/backups" ]; then
		bfail 'the refused run created the backup root inside the store'
		return 1
	fi
	return 0
}

case_15_engine_lock_naming_a_live_pid_is_refused() {
	mkrehearsal lock || { bfail 'fixture setup failed'; return 1; }
	# $$ is this harness: alive for the whole case, so the lock names a live pid.
	printf '{"pid":%d,"startedAt":"2026-10-31T00:00:00Z"}\n' "$$" \
		>"$r/opencode-mem/data/.engine.lock"
	invoke --backup --rehearse "$r" --yes
	if [ "$case_rc" -ne 1 ]; then
		bfail "a store whose .engine.lock names a live pid must be refused (rc 1), got $case_rc"
		return 1
	fi
	if ! printf '%s' "$case_out" | grep -qF 'still owned'; then
		bfail 'the refusal does not say the store is still owned'
		return 1
	fi
	if [ -e "$r/backups/LATEST" ]; then
		bfail 'the refused backup published LATEST'
		return 1
	fi
	return 0
}

case_16_a_non_empty_wal_is_a_refusal_not_a_delete() {
	mkrehearsal wal || { bfail 'fixture setup failed'; return 1; }
	mkdir -p "$r/opencode-mem/data/projects"
	# A stale WAL with no base DB: the checkpoint loop never opens it, so it is
	# still non-empty when the guard counts the WALs. Its content is not a valid
	# WAL frame set on purpose — nothing may consume or delete it.
	printf 'stale wal bytes, never deleted\n' >"$r/opencode-mem/data/projects/stale.db-wal"
	invoke --backup --rehearse "$r" --yes
	if [ "$case_rc" -ne 1 ]; then
		bfail "a non-empty *.db-wal must be refused (rc 1), got $case_rc"
		return 1
	fi
	if ! printf '%s' "$case_out" | grep -qF 'non-empty *.db-wal'; then
		bfail 'the refusal does not name the non-empty *.db-wal'
		return 1
	fi
	if ! printf '%s' "$case_out" | grep -qF 'NOT deleting them'; then
		bfail 'the refusal does not state that the WAL is kept'
		return 1
	fi
	if [ ! -f "$r/opencode-mem/data/projects/stale.db-wal" ]; then
		bfail 'the refusal deleted the non-empty WAL — the non-negotiable is broken'
		return 1
	fi
	if [ -e "$r/backups/LATEST" ]; then
		bfail 'a torn copy was published as a restore point'
		return 1
	fi
	return 0
}

case_17_rollback_refuses_a_backup_that_does_not_verify() {
	mkrehearsal rotten || { bfail 'fixture setup failed'; return 1; }
	"$swap" --backup --rehearse "$r" --yes >/dev/null 2>&1 || { bfail 'the backup failed'; return 1; }
	local ts='' dir='' store_before='' config_before=''
	ts=$(cat "$r/backups/LATEST")
	dir=$r/backups/$ts
	# one byte past the manifest: the directory is no longer a restore point
	printf 'x' >>"$dir/data/metadata.db"
	store_before=$(sha_tree "$r/opencode-mem")
	config_before=$(sha_tree "$r/config")
	invoke --rollback --rehearse "$r" --yes --from "$ts"
	if [ "$case_rc" -ne 1 ]; then
		bfail "a backup that does not verify must be refused (rc 1), got $case_rc"
		return 1
	fi
	case $case_out in
	*'does not verify (§5)'*) ;;
	*)
		bfail 'the refusal does not say the backup failed to verify'
		return 1
		;;
	esac
	if [ "$(sha_tree "$r/opencode-mem")" != "$store_before" ]; then
		bfail 'the refused rollback modified the store'
		return 1
	fi
	if [ "$(sha_tree "$r/config")" != "$config_before" ]; then
		bfail 'the refused rollback modified the config'
		return 1
	fi
	if [ -n "$(find "$r/backups" -maxdepth 1 -type d -name 'data.port-*' -print -quit)" ]; then
		bfail 'the refused rollback moved the port-era store aside'
		return 1
	fi
	return 0
}

case_18_a_pre_reembed_backup_wal_does_not_block_the_backup() {
	mkrehearsal prebaks || { bfail 'fixture setup failed'; return 1; }
	mkdir -p "$r/opencode-mem/data/projects"
	# The engine's own pre-reembed copy (crates/mem-store: reembed_scope leaves
	# `<shard>.db.pre-reembed-<pid>-<ts>.bak` plus a non-empty `.bak-wal` INSIDE
	# data/). It is the engine's artifact, not a torn copy: it must be excluded
	# from the WAL scan and never deleted.
	local bak=$r/opencode-mem/data/projects/project_aaaaaaaaaaaaaaaa_shard_0.db.pre-reembed-1-2
	printf 'pre-reembed wal bytes\n' >"$bak.bak-wal"
	printf 'pre-reembed copy\n' >"$bak.bak"
	invoke --backup --rehearse "$r" --yes
	if [ "$case_rc" -ne 0 ]; then
		bfail "a *.pre-reembed-*.bak-wal must not block the backup (rc 0), got $case_rc: $case_out"
		return 1
	fi
	if [ ! -f "$r/backups/LATEST" ]; then
		bfail 'the backup published no restore point'
		return 1
	fi
	if [ ! -f "$bak.bak-wal" ] || [ ! -f "$bak.bak" ]; then
		bfail 'the backup deleted the engine backup — it must only be excluded from the scan'
		return 1
	fi
	# The exclusion must not widen: a genuine non-empty *.db-wal is still a
	# refusal, and it is still not deleted.
	printf 'stale wal bytes, never deleted\n' >"$r/opencode-mem/data/projects/stale.db-wal"
	invoke --backup --rehearse "$r" --yes
	if [ "$case_rc" -ne 1 ]; then
		bfail "a genuine non-empty *.db-wal must still be refused (rc 1), got $case_rc"
		return 1
	fi
	if ! printf '%s' "$case_out" | grep -qF 'non-empty *.db-wal'; then
		bfail 'the refusal does not name the non-empty *.db-wal'
		return 1
	fi
	if [ ! -f "$r/opencode-mem/data/projects/stale.db-wal" ]; then
		bfail 'the refusal deleted the non-empty WAL — the non-negotiable is broken'
		return 1
	fi
	return 0
}

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
			printf '%s\n' "$case_out" | sed -n '1,20p' | sed 's/^/       /'
		fi
	fi
}

run case_01_help_names_every_mode
run case_02_bare_invocation_writes_nothing
run case_03_dry_run_fails_on_a_missing_store
run case_04_swap_replaces_one_active_line_only
run case_05_swap_refuses_a_double_edit
run case_06_swap_is_idempotent
run case_07_swap_refuses_an_unrelated_config
run case_08_rehearse_refuses_a_live_path
run case_09_backup_writes_a_verified_manifest
run case_10_the_manifest_is_behavioural
run case_11_rollback_refuses_a_post_swap_backup
run case_12_rollback_restores_bytes_and_keeps_the_port_era_store
run case_13_backup_does_not_publish_a_bad_restore_point
run case_14_backup_root_must_live_outside_the_store
run case_15_engine_lock_naming_a_live_pid_is_refused
run case_16_a_non_empty_wal_is_a_refusal_not_a_delete
run case_17_rollback_refuses_a_backup_that_does_not_verify
run case_18_a_pre_reembed_backup_wal_does_not_block_the_backup

printf 'wall time: %ss\n' "$SECONDS"
if [ "$ok" -eq "$total" ]; then
	printf 'PASS %d/%d\n' "$ok" "$total"
else
	printf 'FAIL %d/%d\n' "$ok" "$total"
	exit 1
fi

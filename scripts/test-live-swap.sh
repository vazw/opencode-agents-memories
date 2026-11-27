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
# Exit:  0 when all 21 cases pass, 1 otherwise.
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
total=21
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

# ── helpers for the --verify probe cases ──────────────────────────────────
# lib_call <fn> [arg] — source live-swap.sh in a fresh bash (its
# `[ "${BASH_SOURCE[0]}" = "$0" ]` guard keeps every mode from running) and
# call one function. The absolute "$swap", because this harness resolves
# repo_root from a possibly different cwd. Sets lib_out (stdout+stderr) and
# lib_rc.
lib_out=
lib_rc=0
lib_call() {
	local fn=$1 arg=${2-}
	lib_out=$(bash -c '. "$1"; "$2" "${3-}"' _ "$swap" "$fn" "$arg" 2>&1)
	lib_rc=$?
	return 0
}

# verify_copy_dirs — the live-swap-verify.* dirs under the script's own .tmp
# (REPO_ROOT/.tmp, derived from the script path, never from this harness's cwd).
verify_copy_dirs() {
	local root=''
	root=$(cd -- "$(dirname -- "$swap")/.." && pwd -P) || return 0
	[ -d "$root/.tmp" ] || return 0
	find "$root/.tmp" -maxdepth 1 -mindepth 1 -name 'live-swap-verify.*' -print | sort
}

# stop_bg <pid>... — kill each fixture pid that is still alive. Checked via
# /proc, so an already-exited pid is skipped without a stderr suppresser.
stop_bg() {
	local pid=''
	for pid in "$@"; do
		[ -n "$pid" ] || continue
		[ -d "/proc/$pid" ] || continue
		kill "$pid" || printf 'test-live-swap: kill %s failed\n' "$pid" >&2
	done
	return 0
}

# invoke_timeout <secs> <args...> — run the script under timeout(1). Exit 124 is
# named by the caller as a timed-out invocation, never asserted as missing output.
invoke_timeout() {
	local secs=$1
	shift
	case_out=$(timeout "$secs" "$swap" "$@" 2>&1)
	case_rc=$?
	return 0
}

# cleanup_case21 <aged> <fresh> — remove both sweep fixture dirs on every return
# path. The aged one is already gone on the pass path; [ -d ] skips it. One named
# dir at a time with find -delete, never a recursive remove.
cleanup_case21() {
	local d=''
	for d in "$@"; do
		[ -d "$d" ] || continue
		find "$d" -depth -delete || printf 'test-live-swap: cannot remove %s\n' "$d" >&2
	done
	return 0
}

case_19_verify_reads_versions_from_a_copy_not_the_live_db() {
	# The probe used to open the LIVE db for PRAGMA user_version; under the
	# running engine's exclusive lock that answers `database is locked`, not a
	# version. These rows pin the copy reader (db plus -wal) and the touched
	# predicate. They source the script through its BASH_SOURCE guard, so no
	# mode runs.
	local dir=$work/case19 db='' wal_db='' lock_pid='' writer_pid='' i=0 fired=0
	local only='' walv='' baseline='' after='' noperm='' live=''
	if [ -d "$dir" ]; then
		find "$dir" -depth -delete
	fi
	# the parent run_verify_mode's mkdir -p creates; a direct call needs it
	mkdir -p "$dir" "$(dirname -- "$swap")/../.tmp" || { bfail 'fixture setup failed'; return 1; }
	baseline=$(verify_copy_dirs)
	db=$dir/locked.db
	sqlite3 "$db" 'CREATE TABLE t(x); PRAGMA user_version=4;' >/dev/null || { bfail 'fixture setup failed'; return 1; }

	# (a) a live exclusive lock: the fixture must fire, then the helper reads 4.
	{ printf 'PRAGMA locking_mode=EXCLUSIVE;\nBEGIN EXCLUSIVE;\n.shell sleep 120\n'; } | sqlite3 "$db" >/dev/null &
	lock_pid=$!
	for ((i = 0; i < 50; i++)); do
		# capture, never `sqlite3 | grep -q`: pipefail makes that pipeline
		# non-zero when sqlite exits 1 on the lock even though grep matched.
		live=$(sqlite3 -readonly "$db" 'PRAGMA user_version;' 2>&1)
		case $live in
		*'database is locked'*)
			fired=1
			break
			;;
		esac
		sleep 0.1
	done
	if [ "$fired" -ne 1 ]; then
		stop_bg "$lock_pid"
		bfail 'the exclusive fixture never made a live read answer database is locked'
		return 1
	fi
	lib_call db_user_version "$db"
	if [ "$lib_out" != 4 ]; then
		stop_bg "$lock_pid"
		bfail "the copy reader answered '$lib_out' under the exclusive lock, expected 4"
		return 1
	fi
	stop_bg "$lock_pid"

	# (b) the WAL carries the committed version: the header stays 3, the WAL 4.
	wal_db=$dir/wal.db
	sqlite3 "$wal_db" 'PRAGMA journal_mode=WAL; CREATE TABLE t(x); PRAGMA user_version=3; PRAGMA wal_checkpoint(TRUNCATE);' >/dev/null ||
		{ bfail 'fixture setup failed'; return 1; }
	{ printf 'PRAGMA user_version=4;\n'; sleep 120; } | sqlite3 "$wal_db" &
	writer_pid=$!
	fired=0
	for ((i = 0; i < 50; i++)); do
		# the -wal appears at the writer's first commit; copying it too early
		# fails, so skip (never break) and retry. The stale -shm of a previous
		# read is removed with the pair: two named files, never a glob delete.
		if [ -f "$wal_db-wal" ]; then
			rm -f -- "$dir/wal-with.db" "$dir/wal-with.db-wal" "$dir/wal-with.db-shm"
			if cp -a --reflink=auto -- "$wal_db" "$dir/wal-with.db" &&
				cp -a --reflink=auto -- "$wal_db-wal" "$dir/wal-with.db-wal"; then
				walv=$(sqlite3 -readonly "$dir/wal-with.db" 'PRAGMA user_version;' 2>&1)
				if [ "$walv" = 4 ]; then
					fired=1
					break
				fi
			fi
		fi
		sleep 0.1
	done
	if [ "$fired" -ne 1 ]; then
		stop_bg "$writer_pid"
		bfail "the WAL fixture never carried version 4 on disk (read '$walv')"
		return 1
	fi
	cp -a --reflink=auto -- "$wal_db" "$dir/wal-only.db" || { stop_bg "$writer_pid"; bfail 'fixture setup failed'; return 1; }
	only=$(sqlite3 -readonly "$dir/wal-only.db" 'PRAGMA user_version;' 2>&1)
	if [ "$only" != 3 ]; then
		stop_bg "$writer_pid"
		bfail "a db-only copy read '$only', not the stale header 3 — the fixture does not discriminate"
		return 1
	fi
	lib_call db_user_version "$wal_db"
	if [ "$lib_out" != 4 ]; then
		stop_bg "$writer_pid"
		bfail "the copy reader answered '$lib_out' for a db whose WAL carries 4, expected 4"
		return 1
	fi
	stop_bg "$writer_pid"

	# (c) the touched predicate, both directions: 0 is untouched, 4 is current.
	# Without the second assertion an absent version_is_current (exit 127) would
	# satisfy the first one vacuously.
	lib_call version_is_current 0
	if [ "$lib_rc" -eq 0 ]; then
		bfail 'version_is_current 0 answered current — an untouched DB would pass'
		return 1
	fi
	lib_call version_is_current 4
	if [ "$lib_rc" -ne 0 ]; then
		bfail "version_is_current 4 exited $lib_rc ($lib_out) — absent or always-false is not the rule"
		return 1
	fi

	# (d) an unreadable file is a non-integer token, not a version.
	printf 'not a database\n' >"$dir/bad.db"
	lib_call db_user_version "$dir/bad.db"
	if [ "$lib_rc" -ne 0 ]; then
		bfail "the copy reader aborted (rc $lib_rc) on a corrupt file: $lib_out"
		return 1
	fi
	case $lib_out in
	'' | *[!0-9]*)
		lib_call version_is_current "$lib_out"
		if [ "$lib_rc" -eq 0 ]; then
			bfail "the predicate accepted the error token '$lib_out'"
			return 1
		fi
		;;
	*)
		bfail "the copy reader answered '$lib_out' for a corrupt file, expected an error token"
		return 1
		;;
	esac

	# (e) a copy failure must not abort the run (an unguarded cp/mktemp under
	# set -e would). Root bypasses the permission denial, so require non-root.
	if [ "$(id -u)" -eq 0 ]; then
		bfail 'row (e) needs a non-root runner (root bypasses chmod 000)'
		return 1
	fi
	noperm=$dir/unreadable.db
	sqlite3 "$noperm" 'PRAGMA user_version=4;' >/dev/null || { bfail 'fixture setup failed'; return 1; }
	chmod 000 "$noperm"
	lib_call db_user_version "$noperm"
	chmod 600 "$noperm"
	if [ "$lib_rc" -ne 0 ]; then
		bfail "the copy reader aborted (rc $lib_rc) on an unreadable file: $lib_out"
		return 1
	fi
	case $lib_out in
	'' | *[!0-9]*) ;;
	*)
		bfail "the copy reader answered '$lib_out' for an unreadable file"
		return 1
		;;
	esac

	# (f) five calls leave no new copy behind. A dir an operator's own run owns
	# is not ours to judge, so the comparison is against this case's baseline.
	for ((i = 0; i < 5; i++)); do
		lib_call db_user_version "$db"
	done
	after=$(verify_copy_dirs)
	if [ "$after" != "$baseline" ]; then
		bfail "the copy reader left residue: baseline [$baseline] after [$after]"
		return 1
	fi
	return 0
}

# ── case 20 fixtures: an HTTP stub and a fake engine ──────────────────────
# start_stub <state-dir> — a node HTTP stub on an ephemeral port (listen(0):
# never 3742 or 3749, the live engine's and a prior rehearsal's). Its port lands
# in <dir>/port and is read under a deadline (150 x 0.1 s), so a stub that never
# reports one is a named bfail, never a block. <dir>/mode selects 'ok', 'wrong'
# (the canary is not in its own scoped page) or 'hang' (accept, never answer).
# Sets STUB_PID and STUB_PORT.
STUB_PID=
STUB_PORT=
ENGINE_PID=
start_stub() {
	local dir=$1 i=0 got=''
	: >"$dir/requests"
	cat >"$dir/stub.js" <<'JS'
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const dir = process.argv[2];
const read = (name, dflt) => {
  try { return fs.readFileSync(path.join(dir, name), 'utf8').trim(); } catch (e) { return dflt; }
};
// `pos` is the scoped page: the row this run stored, then six older rows.
const pos = [{ id: 'canary-1', similarity: 0.9 }].concat(
  Array.from({ length: 6 }, (_, i) => ({ id: 'prior-' + i, similarity: 0.5 })));
// `wrong` carries the same CONTENT under another id: only an exact-id match passes.
const wrongPos = () => Array.from({ length: 6 }, (_, i) => ({ id: 'prior-' + i, similarity: 0.5 })).concat(
  [{ id: 'decoy-1', similarity: 0.5, content: read('canary', '') }]);
const server = http.createServer((req, res) => {
  const url = new URL(req.url, 'http://127.0.0.1');
  fs.appendFileSync(path.join(dir, 'requests'), req.method + ' ' + url.pathname + url.search + '\n');
  const mode = read('mode', 'ok');
  if (mode === 'hang') { return; }
  const send = (code, body) => {
    res.writeHead(code, { 'content-type': 'application/json' });
    res.end(JSON.stringify(body));
  };
  const tag = url.searchParams.get('tag') || '';
  const posted = read('tag', '');
  const items = mode === 'wrong' ? wrongPos() : pos;
  // An OMITTED tag models the unfiltered set (measured: dropping `tag` answers
  // the global set), so a negative probe that forgets tag=NEG_TAG reds on
  // behaviour alone. Only a present tag other than `posted` is empty.
  const scoped = tag === '' || tag === posted;
  if (url.pathname === '/api/health') { return send(200, { success: true, status: 'ok' }); }
  if (url.pathname === '/api/stats') { return send(401, { success: false }); }
  if (url.pathname === '/api/memories' && req.method === 'POST') {
    if (mode === 'addfail') { return send(500, { success: false }); }
    let body = '';
    req.on('data', (c) => { body += c; });
    req.on('end', () => {
      try {
        const sent = JSON.parse(body);
        fs.writeFileSync(path.join(dir, 'tag'), sent.containerTag || '');
        fs.writeFileSync(path.join(dir, 'canary'), sent.content || '');
      } catch (e) { }
      send(200, { success: true, data: { id: 'canary-1' } });
    });
    return;
  }
  if (url.pathname === '/api/memories') {
    // The healthy listing demands the tag AND includePrompts=false. Stricter
    // than the engine (a tags filter already drops prompts) on purpose: an
    // omission of either parameter must red, not pass.
    const prompts = url.searchParams.get('includePrompts');
    const listed = mode === 'alltags' || (scoped && (tag === '' || prompts === 'false'));
    return send(200, { success: true, data: { items: listed ? items : [], total: listed ? items.length : 0 } });
  }
  if (url.pathname === '/api/search') {
    if (mode === 'searchfail') { return send(500, { success: false }); }
    const q = url.searchParams.get('q') || '';
    if (!q.startsWith('live-swap canary')) {
      return send(200, { success: true, data: { items: [{ id: 'unrelated-1', similarity: 0.7 }], total: 1 } });
    }
    // A server that ignores the tag: both negatives still answer, and the
    // search total is 20 (the shape of the measured global set).
    if (mode === 'alltags') { return send(200, { success: true, data: { items: pos, total: 20 } }); }
    return send(200, { success: true, data: { items: scoped ? items : [], total: scoped ? items.length : 0 } });
  }
  send(404, { success: false });
});
server.listen(0, '127.0.0.1', () => { console.log(server.address().port); });
JS
	node "$dir/stub.js" "$dir" >"$dir/port" 2>"$dir/stub.err" &
	STUB_PID=$!
	for ((i = 0; i < 150; i++)); do
		if [ -s "$dir/port" ]; then
			got=$(cat "$dir/port")
			case $got in
			'' | *[!0-9]*) ;;
			*)
				STUB_PORT=$got
				return 0
				;;
			esac
		fi
		sleep 0.1
	done
	bfail 'the stub did not report a port'
	return 1
}

# start_fake_engine <data-dir> <web-port> — argv[0] must be the engine name or
# scan_procs cannot see it (a shebang script shows /bin/bash). The trailing `:`
# keeps bash from exec-optimising `sleep 120` away, which would rename the
# process to `sleep` (measured). 120 s outlives --verify's worst case of ~6
# probes x curl --max-time 20.
start_fake_engine() {
	exec -a opencode-agents-memory bash -c 'sleep 120; :' --storage-root "$1" --web-port "$2" &
	ENGINE_PID=$!
}

case_20_verify_canary_search_is_scoped_to_its_tag() {
	# The canary search must run INSIDE the tag the row was stored in: untagged
	# search merges 37 shards and the gibberish canary never enters the global
	# top page (under e5 it is rank 3 of 201 inside its own tag). Rows (f) and
	# (g) of the plan are helper failure paths, not in-suite assertions — the
	# suite cannot spend 60 s on a timeout and an aged-port stub cannot be a row
	# that stays green; the deliberate-break runs exercise them. Do not add them
	# here: they cannot be made to pass inside a full suite run.
	local r=$work/verify20 dir=$work/verify20-stub data='' data_real='' mk=''
	mkrehearsal verify || { bfail 'fixture setup failed'; return 1; }
	if [ -d "$dir" ]; then
		find "$dir" -depth -delete
	fi
	mkdir -p "$dir" || { bfail 'fixture setup failed'; return 1; }
	printf '%s\n' 'ok' >"$dir/mode"

	data=$r/opencode-mem/data
	data_real=$(cd "$data" && pwd -P) || { bfail 'fixture setup failed'; return 1; }
	printf '%s\n' '0000000000000000000000000000000000000000000000000000000000000000' >"$data/.auth-token"
	sqlite3 "$data/metadata.db" 'PRAGMA user_version=2;' >/dev/null || { bfail 'fixture setup failed'; return 1; }
	sqlite3 "$data/user-prompts.db" 'CREATE TABLE p(x); PRAGMA user_version=4;' >/dev/null ||
		{ bfail 'fixture setup failed'; return 1; }
	mkdir -p "$data/projects" || { bfail 'fixture setup failed'; return 1; }
	mk='CREATE TABLE t(x); PRAGMA user_version=4;'
	sqlite3 "$data/projects/project_aaaaaaaaaaaaaaaa.db" "$mk" >/dev/null ||
		{ bfail 'fixture setup failed'; return 1; }

	start_stub "$dir" || { stop_bg "$STUB_PID"; return 1; }
	start_fake_engine "$data_real" "$STUB_PORT"
	if ! case_20_rows "$r" "$data" "$dir"; then
		stop_bg "$STUB_PID" "$ENGINE_PID"
		return 1
	fi
	stop_bg "$STUB_PID" "$ENGINE_PID"
	return 0
}

# case_20_rows <rehearsal> <data> <stub state dir> — rows (a)-(e) of case 20.
# The caller owns the fixture processes and stops them on both return paths.
case_20_rows() {
	local r=$1 data=$2 dir=$3 out='' posted=''
	invoke_timeout 60 --verify --rehearse "$r"
	out=$case_out
	if [ "$case_rc" -eq 124 ]; then
		bfail 'the --verify invocation timed out'
		return 1
	fi

	# (a) the row is found inside its own tag; (d) the real versions are named.
	if [ "$case_rc" -ne 0 ]; then
		bfail "the --verify run with a matching tag expected rc 0, got $case_rc"
		return 1
	fi
	case $out in
	*'[ ok ] the canary is findable'*)
		bfail 'the rank-based canary leg survived — the gate must use the listing, not a rank'
		return 1
		;;
	*) ;;
	esac
	# (a) the row this run stored is in its own tag's listing — exact id, page 1.
	case $out in
	*'[ ok ] the canary row canary-1 is listed under opencode_project_4182e0858e0c93a1 (total 7, first 200 entries)'*) ;;
	*)
		bfail 'the canary row was not reported in its own tag listing'
		return 1
		;;
	esac
	# the other three healthy legs. Sited here, on the healthy run, before `out`
	# is reassigned to the `wrong` run below — that run carries no `the canary row`.
	case $out in
	*'[ ok ] the listing with tag opencode_project_0000000000000000 is empty (total 0) — the tag filter scopes the listing'*) ;;
	*)
		bfail 'the listing tag-filter control did not report the negative tag empty'
		return 1
		;;
	esac
	case $out in
	*'[ ok ] search is scoped to opencode_project_4182e0858e0c93a1: total 7'*) ;;
	*)
		bfail 'the scoped search leg did not report its total'
		return 1
		;;
	esac
	case $out in
	*'[ ok ] the tag filter is honored: tag opencode_project_0000000000000000 answered total 0 (memory entries)'*) ;;
	*)
		bfail 'the search tag-filter control did not report the negative tag empty'
		return 1
		;;
	esac
	# the accumulation truth: one row per --verify, nothing prunes it, and the
	# gate never depends on where that row ranks.
	case $out in
	*'[info] each --verify stores one more canary row'*) ;;
	*)
		bfail 'the accumulation info line is missing'
		return 1
		;;
	esac
	case $out in
	*'the canary is one real memory row'*)
		bfail 'the old one-row canary info wording survived'
		return 1
		;;
	*) ;;
	esac
	case $out in
	*'failures=0'*) ;;
	*)
		bfail 'the matching-tag run reported failures, expected failures=0'
		return 1
		;;
	esac
	case $out in
	*'[ ok ] metadata.db is version 2'*) ;;
	*)
		bfail 'metadata.db was not reported at its real version 2'
		return 1
		;;
	esac
	case $out in
	*'[ ok ] user-prompts.db is version 4 (known max 4)'*) ;;
	*)
		bfail 'user-prompts.db was not reported at its real version 4'
		return 1
		;;
	esac
	case $out in
	*'[ ok ] all 1 project shards have been touched (observed versions: 4)'*) ;;
	*)
		bfail 'the touched-shard line is missing or does not name the real version'
		return 1
		;;
	esac
	# the rank note is negative output: assert the search probe line too, or a
	# deleted probe would satisfy it vacuously.
	case $out in
	*'search works:'*) ;;
	*)
		bfail 'the search probe line is missing'
		return 1
		;;
	esac
	case $out in
	*'rank 1 ≈0.54'*)
		bfail 'the nomic-era rank note survived'
		return 1
		;;
	*) ;;
	esac
	# the tag reached the wire: the stub recorded the POSTed tag, so the search
	# must carry exactly that tag.
	posted=$(cat "$dir/tag" 2>&1)
	if [ -z "$posted" ]; then
		bfail 'the add canary carried no container tag'
		return 1
	fi
	if ! grep -qF "tag=$posted" "$dir/requests"; then
		bfail "the canary search did not carry the tag the row was stored in ($posted)"
		return 1
	fi
	# the positive listing must be scoped on the wire. Here, not at :987: `posted`
	# is assigned above, so this greps the real tag instead of `tag=`.
	if ! grep -qF "GET /api/memories?tag=$posted&pageSize=200&includePrompts=false" "$dir/requests"; then
		bfail 'the positive listing did not carry tag + pageSize=200 + includePrompts=false'
		return 1
	fi

	# (b) a row absent from its own scoped page is a FAIL.
	printf '%s\n' 'wrong' >"$dir/mode"
	invoke_timeout 60 --verify --rehearse "$r"
	out=$case_out
	if [ "$case_rc" -eq 124 ]; then
		bfail 'the --verify invocation timed out'
		return 1
	fi
	case $out in
	*'[FAIL] the canary row canary-1 is not in the first 200 listing entries of opencode_project_4182e0858e0c93a1 (total 7)'*) ;;
	*)
		bfail 'a canary absent from its own listing page was not reported FAIL'
		return 1
		;;
	esac
	case $out in
	*'[ ok ] the canary is findable'*)
		bfail 'the canary was reported findable although it was not in its own tag'
		return 1
		;;
	*) ;;
	esac
	printf '%s\n' 'ok' >"$dir/mode"

	# (a) a failed add is a FAIL line, never a green run; and with no id there is
	# no listing leg to report.
	printf '%s\n' 'addfail' >"$dir/mode"
	invoke_timeout 60 --verify --rehearse "$r"
	out=$case_out
	if [ "$case_rc" -eq 124 ]; then
		bfail 'the --verify invocation timed out'
		return 1
	fi
	case $out in
	*'[FAIL] add answered HTTP 500'*) ;;
	*)
		bfail 'a failed add was not reported as a FAIL line'
		return 1
		;;
	esac
	case $out in
	*'[ ok ] the canary row'*)
		bfail 'a canary row was reported listed although the add failed'
		return 1
		;;
	*) ;;
	esac
	printf '%s\n' 'ok' >"$dir/mode"

	# (d) a server that ignores the tag reds BOTH filter controls: the listing
	# answers 7 and the search 20 for every tag.
	printf '%s\n' 'alltags' >"$dir/mode"
	invoke_timeout 60 --verify --rehearse "$r"
	out=$case_out
	if [ "$case_rc" -eq 124 ]; then
		bfail 'the --verify invocation timed out'
		return 1
	fi
	case $out in
	*'[FAIL] the listing with tag opencode_project_0000000000000000 answered total 7 (expected 0) — the tag filter is not honored in the listing'*) ;;
	*)
		bfail 'an unfiltered listing was not reported FAIL'
		return 1
		;;
	esac
	case $out in
	*'[FAIL] the tag filter is not honored: tag opencode_project_0000000000000000 answered total 20 (expected 0) — an ignored tag would merge other scopes'*) ;;
	*)
		bfail 'an unfiltered search was not reported FAIL'
		return 1
		;;
	esac

	# (c) a broken search path is a FAIL line, not a silent skip.
	printf '%s\n' 'searchfail' >"$dir/mode"
	invoke_timeout 60 --verify --rehearse "$r"
	out=$case_out
	if [ "$case_rc" -eq 124 ]; then
		bfail 'the --verify invocation timed out'
		return 1
	fi
	case $out in
	*'[FAIL] search with tag=opencode_project_4182e0858e0c93a1 answered HTTP 500'*) ;;
	*)
		bfail 'a 500 from the search path was not reported FAIL'
		return 1
		;;
	esac
	printf '%s\n' 'ok' >"$dir/mode"

	# (c) an untouched metadata.db is a FAIL line, not a warn.
	sqlite3 "$data/metadata.db" 'PRAGMA user_version=0;' >/dev/null || { bfail 'fixture setup failed'; return 1; }
	invoke_timeout 60 --verify --rehearse "$r"
	out=$case_out
	if [ "$case_rc" -eq 124 ]; then
		bfail 'the --verify invocation timed out'
		return 1
	fi
	case $out in
	*"[FAIL] metadata.db answered version '0'"*) ;;
	*)
		bfail 'a metadata.db at version 0 was not reported as a FAIL line'
		return 1
		;;
	esac
	sqlite3 "$data/metadata.db" 'PRAGMA user_version=2;' >/dev/null || { bfail 'fixture setup failed'; return 1; }

	# (e) a positive version below this build's max warns, and the aggregate
	# still says "touched" with the sorted-unique observed versions.
	mk='CREATE TABLE t(x); PRAGMA user_version=3;'
	sqlite3 "$data/projects/project_bbbbbbbbbbbbbbbb.db" "$mk" >/dev/null ||
		{ bfail 'fixture setup failed'; return 1; }
	invoke_timeout 60 --verify --rehearse "$r"
	out=$case_out
	if [ "$case_rc" -eq 124 ]; then
		bfail 'the --verify invocation timed out'
		return 1
	fi
	case $out in
	*'[warn] project shard project_bbbbbbbbbbbbbbbb.db is version 3 (< 4)'*) ;;
	*)
		bfail 'a below-max shard was not warned about'
		return 1
		;;
	esac
	case $out in
	*'[ ok ] all 2 project shards have been touched (observed versions: 3 4)'*) ;;
	*)
		bfail 'the touched-shard aggregate stopped naming the sorted-unique versions'
		return 1
		;;
	esac
	case $out in
	*'[ ok ] metadata.db is version 2'*) ;;
	*)
		bfail 'metadata.db lost its real-version line'
		return 1
		;;
	esac
	return 0
}

case_21_verify_sweeps_only_stale_verify_copies() {
	# A killed --verify can leave one live-swap-verify.* dir. The next run must
	# reclaim only dirs older than an hour; a single `find -maxdepth 1 -delete`
	# never descends and deletes nothing, so this rows the loop. Engine-less: the
	# run exits 3 after the sweep (not asserted).
	local r=$work/sweep21 root='' copy_root='' aged='' fresh='' baseline='' after='' expected=''
	mkrehearsal sweep || { bfail 'fixture setup failed'; return 1; }
	root=$(cd -- "$(dirname -- "$swap")/.." && pwd -P) || { bfail 'cannot resolve the script root'; return 1; }
	copy_root=$root/.tmp
	mkdir -p "$copy_root" || { bfail 'fixture setup failed'; return 1; }
	baseline=$(verify_copy_dirs)
	aged=$copy_root/live-swap-verify.dead-$$
	fresh=$copy_root/live-swap-verify.live-$$
	mkdir -p "$aged" "$fresh" || { bfail 'cannot create the fixture copy dirs'; return 1; }
	printf 'leftover bytes\n' >"$aged/leftover"
	touch -d '2 hours ago' "$aged"
	invoke --verify --rehearse "$r"
	if [ -e "$aged" ]; then
		cleanup_case21 "$aged" "$fresh"
		bfail 'the aged verify copy survived the run'
		return 1
	fi
	if [ ! -e "$fresh" ]; then
		cleanup_case21 "$aged" "$fresh"
		bfail 'the fresh verify copy was swept — a concurrent verify owns it'
		return 1
	fi
	after=$(verify_copy_dirs)
	expected=$(printf '%s\n%s\n' "$baseline" "$fresh" | sed '/^$/d' | sort)
	if [ "$after" != "$expected" ]; then
		cleanup_case21 "$aged" "$fresh"
		bfail "the verify copy root is not baseline + this run: expected [$expected] got [$after]"
		return 1
	fi
	cleanup_case21 "$aged" "$fresh"
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
run case_19_verify_reads_versions_from_a_copy_not_the_live_db
run case_20_verify_canary_search_is_scoped_to_its_tag
run case_21_verify_sweeps_only_stale_verify_copies

printf 'wall time: %ss\n' "$SECONDS"
if [ "$ok" -eq "$total" ]; then
	printf 'PASS %d/%d\n' "$ok" "$total"
else
	printf 'FAIL %d/%d\n' "$ok" "$total"
	exit 1
fi
